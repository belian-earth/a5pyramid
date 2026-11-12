//! Partial levels: the exact sums behind a mean, for leaves split over
//! separate builds whose coarse levels span them (a store's partitions, each
//! built alone). [`partials`] reduces one build's leaves to rows of sums;
//! [`merge_partials`] adds the rows of every build and encodes the means,
//! giving the rows [`crate::build`] would give for all the leaves at once.
//!
//! A mean vector's sums are taken as the builder takes them: the weighted
//! sum of each element's value, lifted by its codec ([`Codec::lift`]). With
//! a codec and integer weights the sums are integers, exact in `f64` below
//! 2^53, so the merged mean does not depend on how the leaves were split or
//! in what order the rows are added.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use arrow_array::cast::AsArray;
use arrow_array::types::{Float64Type, UInt64Type, UInt8Type};
use arrow_array::{
    Array, ArrayRef, FixedSizeListArray, Float64Array, Int8Array, RecordBatch, UInt32Array, UInt64Array, UInt8Array,
};
use arrow_schema::{DataType, Field, Schema};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::build::{check_cells, check_lods, check_vectors, check_weights, sort_leaves, supercell_rows, value_lut};
use crate::cancel::Cancel;
use crate::column::{Codec, ColumnType, Reducer, WeightsSpec};
use crate::error::{invalid, Error, Result};
use crate::{Leaves, A5_INDEX, A5_INDEX_KEY, CELL_COLUMN, LOD_COLUMN, NCELL_COLUMN};

/// Schema metadata key of a partials table: [`PartialsMeta`] as JSON.
pub const PARTIALS_META_KEY: &str = "a5pyramid_partials";
/// The version of the partials table's layout.
pub const PARTIALS_VERSION: u32 = 1;
/// Name of the column holding each row's key: the supercell of its cell at
/// the row's key resolution, the shard the merged row belongs to.
pub const KEY_COLUMN: &str = "key";

/// What a partials table holds, in its schema metadata under
/// [`PARTIALS_META_KEY`]: enough to merge and encode its rows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PartialsMeta {
    pub partials_version: u32,
    pub a5_index: String,
    /// The leaves' resolution.
    pub data_resolution: i32,
    /// The weights' name and the type the merged rows store their sums as;
    /// without weights, every leaf weighs 1 and the mean divides by `ncell`.
    pub weights: Option<PartialsWeights>,
    pub vectors: Vec<PartialsVector>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PartialsWeights {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PartialsVector {
    pub name: String,
    /// The element type of the codes, `int8` or `uint8`.
    #[serde(rename = "type")]
    pub ty: String,
    pub dim: usize,
    pub codec: Option<String>,
}

/// One vector column as the merge reads it.
struct VecInfo {
    name: String,
    ty: ColumnType,
    dim: usize,
    codec: Option<Codec>,
}

impl PartialsMeta {
    fn vec_infos(&self) -> Result<Vec<VecInfo>> {
        self.vectors
            .iter()
            .map(|v| {
                let ty = ColumnType::parse(&v.ty)?;
                if !ty.is_byte() {
                    return Err(invalid(format!("partials vector {:?} must be int8 or uint8, not {}", v.name, v.ty)));
                }
                if v.dim == 0 {
                    return Err(invalid(format!("partials vector {:?} has no elements", v.name)));
                }
                let codec = v.codec.as_deref().map(Codec::parse).transpose()?;
                Ok(VecInfo { name: v.name.clone(), ty, dim: v.dim, codec })
            })
            .collect()
    }

    fn weights_spec(&self) -> Result<Option<WeightsSpec>> {
        self.weights
            .as_ref()
            .map(|w| Ok(WeightsSpec { name: w.name.clone(), ty: ColumnType::parse(&w.ty)? }))
            .transpose()
    }
}

/// The partial rows of `leaves` at each of `lods`: for every cell of a LOD
/// that holds a leaf (the supercell of the leaf at the LOD), its key (the
/// supercell of the cell at the LOD's key resolution), the number of leaves
/// (`ncell`), their summed weights (named as the weights column, with
/// weights) and, for each vector column, the sums of its elements' lifted
/// values (`f64`, one fixed-size list a row, named as the column).
///
/// Every value column must be a vector reduced by `Mean`: other reducers do
/// not merge. With weights, every leaf must weigh more than zero (the
/// builder's plain mean over leaves that all weigh nothing has no partial
/// form). `key_resolutions` holds one resolution for every LOD, or one for
/// all, each no finer than its LOD. Rows come in ascending LOD, then cell.
pub fn partials(leaves: Leaves, lods: &[i32], key_resolutions: &[i32], cancel: &Cancel) -> Result<RecordBatch> {
    let Leaves {
        cells,
        columns,
        vectors,
        weights,
        weights_column,
    } = leaves;
    let n = cells.len();
    if n == 0 {
        return Err(invalid("no cells to take partials of"));
    }
    if n > u32::MAX as usize {
        return Err(invalid(format!("{n} cells is more than a pyramid can hold ({})", u32::MAX)));
    }
    if let Some(c) = columns.first() {
        return Err(invalid(format!(
            "partials hold the sums of mean vectors; column {:?} is not a vector",
            c.spec.name
        )));
    }
    if vectors.is_empty() {
        return Err(invalid("partials need at least one vector column"));
    }
    check_vectors(&vectors, &columns, n)?;
    if let Some(v) = vectors.iter().find(|v| v.spec.reducer != Reducer::Mean) {
        return Err(invalid(format!(
            "partials merge only means: vector column {:?} is reduced by {}",
            v.spec.name,
            v.spec.reducer.as_str()
        )));
    }
    if weights.is_some() != weights_column.is_some() {
        return Err(invalid("partials sum the weights into a column of their own: give its name with the weights"));
    }
    let weights = check_weights(weights, n)?;
    if let Some(w) = &weights {
        if let Some(i) = w.par_iter().position_first(|&x| x <= 0.0) {
            return Err(invalid(format!(
                "every leaf must weigh more than zero for partials that merge exactly, not {} (cell {})",
                w[i],
                i + 1
            )));
        }
    }
    let data_resolution = check_cells(&cells)?;
    let lods = check_lods(lods, data_resolution)?;
    if lods.is_empty() {
        return Err(invalid("partials need at least one LOD"));
    }
    let keys: Vec<i32> = match key_resolutions.len() {
        1 => vec![key_resolutions[0]; lods.len()],
        k if k == lods.len() => key_resolutions.to_vec(),
        k => {
            return Err(invalid(format!(
                "{k} key resolutions for {} LODs: give one for all or one for each",
                lods.len()
            )))
        }
    };
    for (&lod, &kr) in lods.iter().zip(&keys) {
        if kr < 0 || kr > lod {
            return Err(invalid(format!(
                "the key resolution of LOD {lod} must be from 0 to {lod}, not {kr}"
            )));
        }
    }
    cancel.check()?;
    let (cells, _, vectors, weights) = sort_leaves(cells, columns, vectors, weights)?;
    cancel.check()?;

    // ascending LOD, the order of the rows
    let mut order: Vec<usize> = (0..lods.len()).collect();
    order.sort_unstable_by_key(|&k| lods[k]);
    let luts: Vec<[f64; 256]> = vectors.iter().map(|v| value_lut(v.spec.ty, v.spec.codec)).collect();

    let mut out_lod: Vec<u8> = Vec::new();
    let mut out_key: Vec<u64> = Vec::new();
    let mut out_cell: Vec<u64> = Vec::new();
    let mut out_ncell: Vec<u64> = Vec::new();
    let mut out_w: Vec<f64> = Vec::new();
    let mut out_sums: Vec<Vec<f64>> = vec![Vec::new(); vectors.len()];
    for k in order {
        let (lod, kr) = (lods[k], keys[k]);
        let mut pairs = supercell_rows(&cells, lod)?;
        cancel.check()?;
        // (parent, leaf) is a total order: the groups, and the order of the
        // leaves within them, are the same on any thread count
        pairs.par_sort_unstable();
        cancel.check()?;
        let mut starts: Vec<usize> = vec![0];
        for i in 1..pairs.len() {
            if pairs[i].0 != pairs[i - 1].0 {
                starts.push(i);
            }
        }
        starts.push(pairs.len());
        let n_groups = starts.len() - 1;
        let group = |g: usize| &pairs[starts[g]..starts[g + 1]];

        let group_cells: Vec<u64> = (0..n_groups).map(|g| pairs[starts[g]].0).collect();
        let group_keys: Vec<u64> = group_cells
            .par_iter()
            .map(|&c| a5::cell_to_supercell(c, kr))
            .collect::<std::result::Result<_, String>>()
            .map_err(Error::A5)?;
        out_lod.extend(std::iter::repeat_n(lod as u8, n_groups));
        out_key.extend_from_slice(&group_keys);
        out_cell.extend_from_slice(&group_cells);
        out_ncell.extend((0..n_groups).map(|g| (starts[g + 1] - starts[g]) as u64));
        if let Some(w) = &weights {
            let ws: Vec<f64> = (0..n_groups)
                .into_par_iter()
                .map(|g| group(g).iter().map(|&(_, i)| w[i as usize]).sum())
                .collect();
            out_w.extend(ws);
        }
        for (v, (lut, sums)) in vectors.iter().zip(luts.iter().zip(out_sums.iter_mut())) {
            let dim = v.spec.dim;
            let start = sums.len();
            sums.resize(start + n_groups * dim, 0.0);
            sums[start..].par_chunks_mut(dim).enumerate().for_each(|(g, acc)| {
                for &(_, i) in group(g) {
                    let i = i as usize;
                    let wi = weights.as_ref().map_or(1.0, |w| w[i]);
                    let row = &v.values[i * dim..(i + 1) * dim];
                    for (a, &b) in acc.iter_mut().zip(row) {
                        *a += wi * lut[b as usize];
                    }
                }
            });
        }
        cancel.check()?;
    }

    let meta = PartialsMeta {
        partials_version: PARTIALS_VERSION,
        a5_index: A5_INDEX.to_string(),
        data_resolution,
        weights: weights_column.as_ref().map(|w| PartialsWeights {
            name: w.name.clone(),
            ty: w.ty.as_str().to_string(),
        }),
        vectors: vectors
            .iter()
            .map(|v| PartialsVector {
                name: v.spec.name.clone(),
                ty: v.spec.ty.as_str().to_string(),
                dim: v.spec.dim,
                codec: v.spec.codec.map(|c| c.as_str().to_string()),
            })
            .collect(),
    };
    let mut fields = vec![
        Field::new(LOD_COLUMN, DataType::UInt8, false),
        Field::new(KEY_COLUMN, DataType::UInt64, false),
        Field::new(CELL_COLUMN, DataType::UInt64, false),
        Field::new(NCELL_COLUMN, DataType::UInt64, false),
    ];
    let mut arrays: Vec<ArrayRef> = vec![
        Arc::new(UInt8Array::from(out_lod)),
        Arc::new(UInt64Array::from(out_key)),
        Arc::new(UInt64Array::from(out_cell)),
        Arc::new(UInt64Array::from(out_ncell)),
    ];
    if let Some(w) = &weights_column {
        fields.push(Field::new(&w.name, DataType::Float64, false));
        arrays.push(Arc::new(Float64Array::from(out_w)));
    }
    for (v, sums) in vectors.iter().zip(out_sums) {
        let item = Arc::new(Field::new("element", DataType::Float64, false));
        fields.push(Field::new(&v.spec.name, DataType::FixedSizeList(item.clone(), v.spec.dim as i32), false));
        arrays.push(Arc::new(FixedSizeListArray::try_new(
            item,
            v.spec.dim as i32,
            Arc::new(Float64Array::from(sums)),
            None,
        )?));
    }
    let schema = Schema::new(fields).with_metadata(meta_map(&meta)?);
    Ok(RecordBatch::try_new(Arc::new(schema), arrays)?)
}

fn meta_map(meta: &PartialsMeta) -> Result<HashMap<String, String>> {
    let mut map = HashMap::new();
    map.insert(PARTIALS_META_KEY.to_string(), serde_json::to_string(meta)?);
    map.insert(A5_INDEX_KEY.to_string(), A5_INDEX.to_string());
    Ok(map)
}

/// The [`PartialsMeta`] of a partials table's schema.
pub fn partials_meta(schema: &Schema) -> Result<PartialsMeta> {
    let json = schema.metadata().get(PARTIALS_META_KEY).ok_or_else(|| {
        invalid(format!(
            "not a partials table: its schema has no {PARTIALS_META_KEY:?} metadata"
        ))
    })?;
    let meta: PartialsMeta = serde_json::from_str(json)?;
    if meta.partials_version != PARTIALS_VERSION {
        return Err(Error::Format(format!(
            "partials version {}, but this crate reads version {PARTIALS_VERSION}",
            meta.partials_version
        )));
    }
    if meta.a5_index != A5_INDEX {
        return Err(invalid(format!(
            "partials of A5 index {}, not {A5_INDEX}",
            meta.a5_index
        )));
    }
    Ok(meta)
}

/// The rows of partials tables as read, before they are summed.
struct Acc {
    lod: Vec<u8>,
    key: Vec<u64>,
    cell: Vec<u64>,
    ncell: Vec<u64>,
    w: Vec<f64>,
    sums: Vec<Vec<f64>>,
}

/// A merge of partial rows ([`partials`]) of any number of builds of one
/// pyramid, fed batch by batch ([`PartialsMerge::push`]) so that only the
/// rows kept are held, then summed and encoded ([`PartialsMerge::finish`]).
pub struct PartialsMerge {
    meta: PartialsMeta,
    vecs: Vec<VecInfo>,
    wspec: Option<WeightsSpec>,
    names: Vec<String>,
    lod_k: usize,
    key_k: usize,
    cell_k: usize,
    ncell_k: usize,
    w_k: Option<usize>,
    vec_k: Vec<usize>,
    keys: Option<HashSet<u64>>,
    acc: Acc,
}

impl PartialsMerge {
    /// A merge of partials tables of `schema` (a partials table's, with its
    /// [`PARTIALS_META_KEY`] metadata), keeping only rows of `keys` when
    /// given.
    pub fn new(schema: &Schema, keys: Option<HashSet<u64>>) -> Result<Self> {
        let meta = partials_meta(schema)?;
        let vecs = meta.vec_infos()?;
        let wspec = meta.weights_spec()?;
        let index = |name: &str, ty: &DataType| -> Result<usize> {
            let k = schema
                .index_of(name)
                .map_err(|_| invalid(format!("the partials have no column {name:?}")))?;
            if schema.field(k).data_type() != ty {
                return Err(invalid(format!(
                    "partials column {name:?} must be {ty}, not {}",
                    schema.field(k).data_type()
                )));
            }
            Ok(k)
        };
        let lod_k = index(LOD_COLUMN, &DataType::UInt8)?;
        let key_k = index(KEY_COLUMN, &DataType::UInt64)?;
        let cell_k = index(CELL_COLUMN, &DataType::UInt64)?;
        let ncell_k = index(NCELL_COLUMN, &DataType::UInt64)?;
        let w_k = wspec
            .as_ref()
            .map(|w| index(&w.name, &DataType::Float64))
            .transpose()?;
        let vec_k: Vec<usize> = vecs
            .iter()
            .map(|v| {
                let k = schema
                    .index_of(&v.name)
                    .map_err(|_| invalid(format!("the partials have no column {:?}", v.name)))?;
                match schema.field(k).data_type() {
                    DataType::FixedSizeList(f, d) if *d as usize == v.dim && f.data_type() == &DataType::Float64 => Ok(k),
                    other => Err(invalid(format!(
                        "partials column {:?} must be a fixed-size list of {} float64, not {other}",
                        v.name, v.dim
                    ))),
                }
            })
            .collect::<Result<_>>()?;
        let n_vecs = vecs.len();
        Ok(Self {
            meta,
            vecs,
            wspec,
            names: schema.fields().iter().map(|f| f.name().clone()).collect(),
            lod_k,
            key_k,
            cell_k,
            ncell_k,
            w_k,
            vec_k,
            keys,
            acc: Acc {
                lod: Vec::new(),
                key: Vec::new(),
                cell: Vec::new(),
                ncell: Vec::new(),
                w: Vec::new(),
                sums: vec![Vec::new(); n_vecs],
            },
        })
    }

    /// Read a batch's rows (those of the merge's keys, when it has them).
    pub fn push(&mut self, batch: &RecordBatch) -> Result<()> {
        if batch.num_rows() == 0 {
            return Ok(());
        }
        let names: Vec<&String> = batch.schema_ref().fields().iter().map(|f| f.name()).collect();
        if names.len() != self.names.len() || names.iter().zip(&self.names).any(|(a, b)| *a != b) {
            return Err(invalid("a batch of partials whose columns differ from the merge's schema"));
        }
        let mut ks = vec![self.lod_k, self.key_k, self.cell_k, self.ncell_k];
        ks.extend(self.w_k);
        ks.extend(&self.vec_k);
        if ks.iter().any(|&k| batch.column(k).null_count() > 0) {
            return Err(invalid("partial rows have no missing values"));
        }
        let lod = batch.column(self.lod_k).as_primitive::<UInt8Type>().values();
        let key = batch.column(self.key_k).as_primitive::<UInt64Type>().values();
        let rows: Vec<usize> = match &self.keys {
            None => (0..batch.num_rows()).collect(),
            Some(ks) => (0..batch.num_rows()).filter(|&i| ks.contains(&key[i])).collect(),
        };
        if rows.is_empty() {
            return Ok(());
        }
        let cell = batch.column(self.cell_k).as_primitive::<UInt64Type>().values();
        let ncell = batch.column(self.ncell_k).as_primitive::<UInt64Type>().values();
        let acc = &mut self.acc;
        acc.lod.extend(rows.iter().map(|&i| lod[i]));
        acc.key.extend(rows.iter().map(|&i| key[i]));
        acc.cell.extend(rows.iter().map(|&i| cell[i]));
        acc.ncell.extend(rows.iter().map(|&i| ncell[i]));
        if let Some(k) = self.w_k {
            let w = batch.column(k).as_primitive::<Float64Type>().values();
            acc.w.extend(rows.iter().map(|&i| w[i]));
        }
        for ((v, &k), sums) in self.vecs.iter().zip(&self.vec_k).zip(acc.sums.iter_mut()) {
            let list = batch.column(k).as_fixed_size_list();
            let values = list.values().as_primitive::<Float64Type>().values();
            let base = list.offset() * v.dim;
            for &i in &rows {
                let s = base + i * v.dim;
                sums.extend_from_slice(&values[s..s + v.dim]);
            }
        }
        Ok(())
    }

    /// Sum each (LOD, cell)'s rows and encode its means as the builder
    /// encodes them: the codes of the mean of the lifted values. The result
    /// holds the LOD, key, cell, `ncell` (`uint32`), the summed weights (in
    /// the stored weights type) and each vector's codes (a fixed-size list
    /// of its element type), in ascending LOD, key and cell: the rows of one
    /// LOD and key are one shard.
    pub fn finish(self, cancel: &Cancel) -> Result<RecordBatch> {
        merge_rows(self.acc, &self.meta, &self.vecs, self.wspec.as_ref(), cancel)
    }
}

/// [`PartialsMerge`] over `batches` of partials tables of `schema`.
pub fn merge_partials<I>(schema: &Schema, batches: I, keys: Option<&HashSet<u64>>, cancel: &Cancel) -> Result<RecordBatch>
where
    I: IntoIterator<Item = Result<RecordBatch>>,
{
    let mut m = PartialsMerge::new(schema, keys.cloned())?;
    for batch in batches {
        cancel.check()?;
        m.push(&batch?)?;
    }
    cancel.check()?;
    m.finish(cancel)
}

fn merge_rows(
    acc: Acc,
    meta: &PartialsMeta,
    vecs: &[VecInfo],
    wspec: Option<&WeightsSpec>,
    cancel: &Cancel,
) -> Result<RecordBatch> {
    let n = acc.cell.len();
    // rows of one (LOD, cell) together; within them in input order, and the
    // sums are of integers wherever they are exact, so the order is moot
    let mut order: Vec<u32> = (0..n as u32).collect();
    order.par_sort_unstable_by_key(|&i| (acc.lod[i as usize], acc.cell[i as usize], i));
    cancel.check()?;
    let mut starts: Vec<usize> = vec![0];
    for j in 1..n {
        let (a, b) = (order[j - 1] as usize, order[j] as usize);
        if acc.lod[a] != acc.lod[b] || acc.cell[a] != acc.cell[b] {
            starts.push(j);
        }
    }
    if n > 0 {
        starts.push(n);
    }
    let n_groups = starts.len().saturating_sub(1);
    let group = |g: usize| &order[starts[g]..starts[g + 1]];

    for g in 0..n_groups {
        let rows = group(g);
        let first = rows[0] as usize;
        let lod = acc.lod[first] as i32;
        if lod >= meta.data_resolution {
            return Err(invalid(format!(
                "a partial row at LOD {lod}, not below the leaves' resolution {}",
                meta.data_resolution
            )));
        }
        if a5::get_resolution(acc.cell[first]) != lod {
            return Err(invalid(format!(
                "partial row cell {} is not at its LOD {lod}",
                crate::hex(acc.cell[first])
            )));
        }
        if rows.iter().any(|&i| acc.key[i as usize] != acc.key[first]) {
            return Err(invalid(format!(
                "the partial rows of cell {} at LOD {lod} name different keys",
                crate::hex(acc.cell[first])
            )));
        }
    }

    // the output in ascending LOD, key and cell
    let mut out_order: Vec<usize> = (0..n_groups).collect();
    out_order.par_sort_unstable_by_key(|&g| {
        let i = group(g)[0] as usize;
        (acc.lod[i], acc.key[i], acc.cell[i])
    });
    let first = |g: usize| group(g)[0] as usize;
    let lod: Vec<u8> = out_order.iter().map(|&g| acc.lod[first(g)]).collect();
    let key: Vec<u64> = out_order.iter().map(|&g| acc.key[first(g)]).collect();
    let cell: Vec<u64> = out_order.iter().map(|&g| acc.cell[first(g)]).collect();
    let ncell64: Vec<u64> = out_order
        .iter()
        .map(|&g| group(g).iter().map(|&i| acc.ncell[i as usize]).sum())
        .collect();
    if let Some(i) = ncell64.iter().position(|&c| c == 0 || c > u32::MAX as u64) {
        return Err(invalid(format!(
            "cell {} summarises {} leaves: a row holds from 1 to {}",
            crate::hex(cell[i]),
            ncell64[i],
            u32::MAX
        )));
    }
    // the divisor of each mean: the summed weights, or the leaves without
    let wsum: Vec<f64> = match wspec {
        Some(_) => out_order
            .iter()
            .map(|&g| group(g).iter().map(|&i| acc.w[i as usize]).sum())
            .collect(),
        None => ncell64.iter().map(|&c| c as f64).collect(),
    };
    cancel.check()?;

    let mut fields = vec![
        Field::new(LOD_COLUMN, DataType::UInt8, false),
        Field::new(KEY_COLUMN, DataType::UInt64, false),
        Field::new(CELL_COLUMN, DataType::UInt64, false),
        Field::new(NCELL_COLUMN, DataType::UInt32, false),
    ];
    let mut arrays: Vec<ArrayRef> = vec![
        Arc::new(UInt8Array::from(lod)),
        Arc::new(UInt64Array::from(key)),
        Arc::new(UInt64Array::from(cell)),
        Arc::new(UInt32Array::from(ncell64.iter().map(|&c| c as u32).collect::<Vec<u32>>())),
    ];
    if let Some(w) = wspec {
        fields.push(Field::new(&w.name, w.ty.data_type(), false));
        let stored: Vec<f64> = wsum.iter().map(|&x| w.ty.quantise(x)).collect();
        arrays.push(crate::tiled::value_array(w.ty, &stored));
    }
    for (v, sums) in vecs.iter().zip(&acc.sums) {
        let dim = v.dim;
        let mut codes = vec![0u8; n_groups * dim];
        codes.par_chunks_mut(dim).enumerate().for_each_init(
            || vec![0.0; dim],
            |s, (j, out)| {
                let g = out_order[j];
                s.fill(0.0);
                for &i in group(g) {
                    let row = &sums[i as usize * dim..(i as usize + 1) * dim];
                    for (a, &x) in s.iter_mut().zip(row) {
                        *a += x;
                    }
                }
                for (o, &a) in out.iter_mut().zip(s.iter()) {
                    let m = a / wsum[j];
                    let q = match v.codec {
                        Some(c) => c.lower(m),
                        None => v.ty.quantise(m),
                    };
                    *o = match v.ty {
                        ColumnType::Int8 => q as i8 as u8,
                        _ => q as u8,
                    };
                }
            },
        );
        let item = Arc::new(Field::new("element", v.ty.data_type(), false));
        let values: ArrayRef = match v.ty {
            ColumnType::Int8 => Arc::new(Int8Array::from(codes.into_iter().map(|b| b as i8).collect::<Vec<i8>>())),
            _ => Arc::new(UInt8Array::from(codes)),
        };
        fields.push(Field::new(&v.name, DataType::FixedSizeList(item.clone(), dim as i32), false));
        arrays.push(Arc::new(FixedSizeListArray::try_new(item, dim as i32, values, None)?));
        cancel.check()?;
    }
    let mut map = HashMap::new();
    map.insert(A5_INDEX_KEY.to_string(), A5_INDEX.to_string());
    let schema = Schema::new(fields).with_metadata(map);
    Ok(RecordBatch::try_new(Arc::new(schema), arrays)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::column::{VectorColumn, WeightsSpec};
    use crate::testutil::grid;
    use crate::{build, Leaves};

    /// Leaves at resolution 12 over a box, a 4-element AEF vector each and
    /// integer weights, in a fixed pseudo-random pattern.
    fn aef_leaves(cells: &[u64], weighted: bool) -> Leaves {
        let dim = 4;
        let mut bytes = Vec::with_capacity(cells.len() * dim);
        let mut w = Vec::with_capacity(cells.len());
        for (i, &c) in cells.iter().enumerate() {
            for e in 0..dim {
                let x = ((c.wrapping_mul(2654435761).wrapping_add((e * 97 + i) as u64)) % 256) as u8;
                bytes.push(x);
            }
            w.push(1.0 + ((c >> 7) % 9) as f64);
        }
        let mut v = VectorColumn::new("emb", ColumnType::Int8, dim, bytes);
        v.spec.reducer = Reducer::Mean;
        v.spec.codec = Some(Codec::Aef);
        Leaves {
            cells: cells.to_vec(),
            columns: Vec::new(),
            vectors: vec![v],
            weights: weighted.then_some(w),
            weights_column: weighted.then(|| WeightsSpec { name: "n".into(), ty: ColumnType::UInt32 }),
        }
    }

    fn subset(l: &Leaves, keep: &[usize]) -> Leaves {
        let dim = l.vectors[0].spec.dim;
        let mut v = l.vectors[0].clone();
        v.values = keep.iter().flat_map(|&i| l.vectors[0].values[i * dim..(i + 1) * dim].to_vec()).collect();
        Leaves {
            cells: keep.iter().map(|&i| l.cells[i]).collect(),
            columns: Vec::new(),
            vectors: vec![v],
            weights: l.weights.as_ref().map(|w| keep.iter().map(|&i| w[i]).collect()),
            weights_column: l.weights_column.clone(),
        }
    }

    fn merged(parts: Vec<RecordBatch>) -> RecordBatch {
        let schema = parts[0].schema();
        merge_partials(&schema, parts.into_iter().map(Ok), None, &Cancel::new()).unwrap()
    }

    /// The merged rows of one LOD as (cell, ncell, weights, codes).
    fn rows_at(m: &RecordBatch, lod: u8) -> Vec<(u64, u32, Option<u32>, Vec<i8>)> {
        let l = m.column(0).as_primitive::<UInt8Type>().values();
        let c = m.column(2).as_primitive::<UInt64Type>().values();
        let n = m.column(3).as_primitive::<arrow_array::types::UInt32Type>().values();
        let has_w = m.num_columns() == 6;
        let codes = m.column(m.num_columns() - 1).as_fixed_size_list();
        let dim = codes.value_length() as usize;
        let vals = codes.values().as_primitive::<arrow_array::types::Int8Type>().values();
        let mut out: Vec<_> = (0..m.num_rows())
            .filter(|&i| l[i] == lod)
            .map(|i| {
                let w = has_w.then(|| m.column(4).as_primitive::<arrow_array::types::UInt32Type>().value(i));
                (c[i], n[i], w, vals[i * dim..(i + 1) * dim].to_vec())
            })
            .collect();
        out.sort_by_key(|r| r.0);
        out
    }

    /// The built pyramid's rows of one LOD in the same form.
    fn built_at(p: &crate::Pyramid, lod: i32) -> Vec<(u64, u32, Option<u32>, Vec<i8>)> {
        let lv = p.level(lod).unwrap();
        let dim = p.vectors[0].dim;
        (0..lv.len())
            .map(|i| {
                let w = (!lv.weights.is_empty()).then(|| lv.weights[i] as u32);
                let codes = lv.vectors[0][i * dim..(i + 1) * dim].iter().map(|&b| b as i8).collect();
                (lv.cells[i], lv.ncell[i], w, codes)
            })
            .collect()
    }

    #[test]
    fn merged_partials_of_any_split_are_the_built_levels() {
        let cells = grid(12, (8.0, 8.6), (47.0, 47.4), 60);
        for weighted in [true, false] {
            let all = aef_leaves(&cells, weighted);
            let lods = [7, 9, 10];
            let built = build(all.clone(), &lods, &Cancel::new()).unwrap();
            // one build, two halves by supercell at 8, and three interleaved parts
            let by8: Vec<u64> = cells.iter().map(|&c| a5::cell_to_supercell(c, 8).unwrap()).collect();
            let pivot = by8[by8.len() / 2];
            let splits: Vec<Vec<Vec<usize>>> = vec![
                vec![(0..cells.len()).collect()],
                vec![
                    (0..cells.len()).filter(|&i| by8[i] < pivot).collect(),
                    (0..cells.len()).filter(|&i| by8[i] >= pivot).collect(),
                ],
                (0..3).map(|r| (0..cells.len()).filter(|&i| i % 3 == r).collect()).collect(),
            ];
            for split in splits {
                let parts: Vec<RecordBatch> = split
                    .iter()
                    .map(|keep| partials(subset(&all, keep), &lods, &[3], &Cancel::new()).unwrap())
                    .collect();
                let m = merged(parts);
                for &lod in &lods {
                    assert_eq!(rows_at(&m, lod as u8), built_at(&built, lod), "lod {lod}, weighted {weighted}");
                }
            }
        }
    }

    #[test]
    fn keys_are_the_supercells_of_the_cells_and_select_rows() {
        let cells = grid(12, (8.0, 8.6), (47.0, 47.4), 40);
        let p = partials(aef_leaves(&cells, true), &[7, 10], &[2, 5], &Cancel::new()).unwrap();
        let l = p.column(0).as_primitive::<UInt8Type>().values();
        let k = p.column(1).as_primitive::<UInt64Type>().values();
        let c = p.column(2).as_primitive::<UInt64Type>().values();
        for i in 0..p.num_rows() {
            let kr = if l[i] == 7 { 2 } else { 5 };
            assert_eq!(k[i], a5::cell_to_supercell(c[i], kr).unwrap());
        }
        let one: HashSet<u64> = [k[p.num_rows() - 1]].into_iter().collect();
        let schema = p.schema();
        let m = merge_partials(&schema, vec![Ok(p.clone())], Some(&one), &Cancel::new()).unwrap();
        let mk = m.column(1).as_primitive::<UInt64Type>().values();
        assert!(m.num_rows() > 0 && mk.iter().all(|x| one.contains(x)));
        // ascending LOD, key and cell
        let ml = m.column(0).as_primitive::<UInt8Type>().values();
        let mc = m.column(2).as_primitive::<UInt64Type>().values();
        for i in 1..m.num_rows() {
            assert!((ml[i - 1], mk[i - 1], mc[i - 1]) < (ml[i], mk[i], mc[i]));
        }
    }

    #[test]
    fn partials_refuse_what_does_not_merge() {
        let cells = grid(12, (8.0, 8.2), (47.0, 47.2), 10);
        let mut l = aef_leaves(&cells, true);
        l.vectors[0].spec.reducer = Reducer::RepChild;
        l.vectors[0].spec.codec = None;
        assert!(partials(l, &[9], &[3], &Cancel::new()).is_err());
        let mut l = aef_leaves(&cells, true);
        l.weights.as_mut().unwrap()[0] = 0.0;
        assert!(partials(l, &[9], &[3], &Cancel::new()).is_err());
        assert!(partials(aef_leaves(&cells, true), &[9], &[10], &Cancel::new()).is_err());
        assert!(partials(aef_leaves(&cells, true), &[9, 8], &[1, 2, 3], &Cancel::new()).is_err());
        assert!(partials(aef_leaves(&cells, true), &[12], &[3], &Cancel::new()).is_err());
    }

    #[test]
    fn a_table_without_partials_metadata_is_refused() {
        let cells = grid(12, (8.0, 8.2), (47.0, 47.2), 10);
        let p = partials(aef_leaves(&cells, true), &[9], &[3], &Cancel::new()).unwrap();
        let bare = Schema::new(p.schema().fields().clone());
        assert!(merge_partials(&bare, vec![Ok(p)], None, &Cancel::new()).is_err());
    }
}
