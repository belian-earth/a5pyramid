//! Level construction: group the leaves under each coarser LOD by supercell
//! and reduce every column within each group.

use std::collections::HashSet;

use rayon::prelude::*;

use crate::cancel::Cancel;
use crate::column::{Codec, Column, ColumnSpec, ColumnType, Reducer, VectorColumn, VectorSpec};
use crate::error::{invalid, Error, Result};
use crate::RESERVED_COLUMNS;

/// The data a pyramid is built from: cells at one resolution, value columns
/// and vector columns aligned with them, and optional non-negative weights
/// for means, modes and the representative leaf (NaN weights count as
/// zero).
#[derive(Clone, Debug, Default)]
pub struct Leaves {
    pub cells: Vec<u64>,
    pub columns: Vec<Column>,
    pub vectors: Vec<VectorColumn>,
    pub weights: Option<Vec<f64>>,
}

/// One level of detail: its cells in ascending id order, the number of
/// leaves each summarises, and one value vector per column (NaN where
/// missing), already quantised to the column's storage type.
#[derive(Clone, Debug, PartialEq)]
pub struct Level {
    pub lod: i32,
    pub cells: Vec<u64>,
    pub ncell: Vec<u32>,
    pub values: Vec<Vec<f64>>,
    /// One per vector column: `dim` bytes per cell, row by row.
    pub vectors: Vec<Vec<u8>>,
}

impl Level {
    pub fn len(&self) -> usize {
        self.cells.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }
}

/// A built pyramid: the levels in ascending LOD order, the leaves last.
#[derive(Clone, Debug, PartialEq)]
pub struct Pyramid {
    pub data_resolution: i32,
    pub columns: Vec<ColumnSpec>,
    pub vectors: Vec<VectorSpec>,
    pub levels: Vec<Level>,
}

impl Pyramid {
    pub fn n_rows(&self) -> usize {
        self.levels.iter().map(Level::len).sum()
    }

    pub fn lods(&self) -> Vec<i32> {
        self.levels.iter().map(|l| l.lod).collect()
    }

    pub fn level(&self, lod: i32) -> Option<&Level> {
        self.levels.iter().find(|l| l.lod == lod)
    }
}

/// The coarser LODs built by default: every resolution from 2 up to, not
/// including, the data resolution. Resolutions 0 and 1 hold the 12 and 60
/// largest cells, which no map view draws.
pub fn default_lods(data_resolution: i32) -> Vec<i32> {
    (2.min(data_resolution)..data_resolution).collect()
}

/// Build a pyramid from `leaves` with a level at each of `lods`, the
/// resolutions coarser than the leaves' to aggregate to, plus the leaves
/// themselves. A cell at LOD `r` summarises the leaves whose centres it
/// contains ([`a5::cell_to_supercell`]); `ncell` counts them.
pub fn build(leaves: Leaves, lods: &[i32], cancel: &Cancel) -> Result<Pyramid> {
    let Leaves {
        cells,
        columns,
        vectors,
        weights,
    } = leaves;
    let n = cells.len();
    if n == 0 {
        return Err(invalid("no cells to build a pyramid from"));
    }
    if n > u32::MAX as usize {
        return Err(invalid(format!(
            "{n} cells is more than a pyramid can hold ({})",
            u32::MAX
        )));
    }
    check_columns(&columns, n)?;
    check_vectors(&vectors, &columns, n)?;
    let weights = check_weights(weights, n)?;
    let data_resolution = check_cells(&cells)?;
    let mut lods = check_lods(lods, data_resolution)?;
    cancel.check()?;

    let (cells, mut columns, mut vectors, weights) = sort_leaves(cells, columns, vectors, weights)?;
    cancel.check()?;
    for col in &mut columns {
        let ty = col.spec.ty;
        col.values.par_iter_mut().for_each(|v| *v = ty.quantise(*v));
    }

    let rep = RepSpec::new(&columns, &vectors);
    for (&k, &sd) in rep.cols.iter().zip(&rep.sds) {
        columns[k].spec.sd = Some(sd);
    }
    for (&k, &sd) in rep.vecs.iter().zip(&rep.vec_sds) {
        vectors[k].spec.sd = Some(sd);
    }
    // finest first, so the largest groupings (and any error) come early
    lods.sort_unstable_by(|a, b| b.cmp(a));
    let mut levels = Vec::with_capacity(lods.len() + 1);
    for &lod in &lods {
        cancel.check()?;
        levels.push(build_level(
            &cells,
            &columns,
            &vectors,
            weights.as_deref(),
            &rep,
            lod,
            cancel,
        )?);
    }
    levels.reverse();
    let specs = columns.iter().map(|c| c.spec.clone()).collect();
    let vector_specs = vectors.iter().map(|v| v.spec.clone()).collect();
    levels.push(Level {
        lod: data_resolution,
        ncell: vec![1; n],
        values: columns.into_iter().map(|c| c.values).collect(),
        vectors: vectors.into_iter().map(|v| v.values).collect(),
        cells,
    });
    Ok(Pyramid {
        data_resolution,
        columns: specs,
        vectors: vector_specs,
        levels,
    })
}

fn check_columns(columns: &[Column], n: usize) -> Result<()> {
    let mut seen = HashSet::new();
    for col in columns {
        let name = &col.spec.name;
        if name.is_empty() {
            return Err(invalid("value columns need non-empty names"));
        }
        if RESERVED_COLUMNS.contains(&name.as_str()) {
            return Err(invalid(format!(
                "a value column cannot be named {name:?}, which the pyramid uses for its own column"
            )));
        }
        if !seen.insert(name.as_str()) {
            return Err(invalid(format!("value column {name:?} appears more than once")));
        }
        if col.values.len() != n {
            return Err(invalid(format!(
                "value column {name:?} has {} values for {n} cells",
                col.values.len()
            )));
        }
        check_codec(name, col.spec.codec, col.spec.reducer, col.spec.ty)?;
        if let Some(sd) = col.spec.sd {
            if col.spec.reducer != Reducer::RepChild {
                return Err(invalid(format!(
                    "value column {name:?} has a standard deviation, which only rep_child columns use"
                )));
            }
            if !(sd > 0.0 && sd.is_finite()) {
                return Err(invalid(format!(
                    "the standard deviation of value column {name:?} must be positive and finite, not {sd}"
                )));
            }
        }
    }
    Ok(())
}

fn check_vectors(vectors: &[VectorColumn], columns: &[Column], n: usize) -> Result<()> {
    let mut seen: HashSet<&str> = columns.iter().map(|c| c.spec.name.as_str()).collect();
    for v in vectors {
        let name = &v.spec.name;
        if name.is_empty() {
            return Err(invalid("vector columns need non-empty names"));
        }
        if RESERVED_COLUMNS.contains(&name.as_str()) {
            return Err(invalid(format!(
                "a vector column cannot be named {name:?}, which the pyramid uses for its own column"
            )));
        }
        if !seen.insert(name.as_str()) {
            return Err(invalid(format!("column {name:?} appears more than once")));
        }
        if !v.spec.ty.is_byte() {
            return Err(invalid(format!(
                "vector column {name:?} must be int8 or uint8, not {}",
                v.spec.ty.as_str()
            )));
        }
        let dim = v.spec.dim;
        if dim == 0 {
            return Err(invalid(format!(
                "vector column {name:?} needs at least one element a cell"
            )));
        }
        if n.checked_mul(dim) != Some(v.values.len()) {
            return Err(invalid(format!(
                "vector column {name:?} has {} bytes for {n} cells of {dim} elements",
                v.values.len()
            )));
        }
        if !matches!(v.spec.reducer, Reducer::RepChild | Reducer::Mean) {
            return Err(invalid(format!(
                "vector column {name:?} must be reduced by rep_child or mean, not {}",
                v.spec.reducer.as_str()
            )));
        }
        check_codec(name, v.spec.codec, v.spec.reducer, v.spec.ty)?;
        if let Some(sd) = v.spec.sd {
            if v.spec.reducer != Reducer::RepChild {
                return Err(invalid(format!(
                    "vector column {name:?} has a standard deviation, which only rep_child vectors use"
                )));
            }
            if !(sd > 0.0 && sd.is_finite()) {
                return Err(invalid(format!(
                    "the standard deviation of vector column {name:?} must be positive and finite, not {sd}"
                )));
            }
        }
    }
    Ok(())
}

/// A codec decodes codes for a mean, and its column must hold its codes.
fn check_codec(name: &str, codec: Option<Codec>, reducer: Reducer, ty: ColumnType) -> Result<()> {
    let Some(codec) = codec else { return Ok(()) };
    if reducer != Reducer::Mean {
        return Err(invalid(format!(
            "column {name:?} has the codec {}, which applies to means, but is reduced by {}",
            codec.as_str(),
            reducer.as_str()
        )));
    }
    if ty != codec.code_type() {
        return Err(invalid(format!(
            "column {name:?} has the codec {}, whose codes are {}, but is stored as {}",
            codec.as_str(),
            codec.code_type().as_str(),
            ty.as_str()
        )));
    }
    Ok(())
}

fn check_weights(weights: Option<Vec<f64>>, n: usize) -> Result<Option<Vec<f64>>> {
    let Some(mut w) = weights else { return Ok(None) };
    if w.len() != n {
        return Err(invalid(format!("{} weights for {n} cells", w.len())));
    }
    if let Some(i) = w.par_iter().position_first(|&x| x < 0.0 || x.is_infinite()) {
        return Err(invalid(format!(
            "weights must be finite and not negative, not {} (cell {})",
            w[i],
            i + 1
        )));
    }
    w.par_iter_mut().for_each(|x| {
        if x.is_nan() {
            *x = 0.0
        }
    });
    Ok(Some(w))
}

/// The cells' common resolution, after checking that each is a cell (not
/// the world cell or a compaction marker).
fn check_cells(cells: &[u64]) -> Result<i32> {
    let bad = cells
        .par_iter()
        .position_first(|&c| c == a5::WORLD_CELL || a5::is_compaction_marker(c) || !a5::is_valid_cell(c));
    if let Some(i) = bad {
        return Err(invalid(format!(
            "cell {} ({}) is not an A5 cell",
            i + 1,
            crate::hex(cells[i])
        )));
    }
    let res = a5::get_resolution(cells[0]);
    if let Some(i) = cells.par_iter().position_first(|&c| a5::get_resolution(c) != res) {
        return Err(invalid(format!(
            "a pyramid needs cells at one resolution, but cell 1 is at {res} and cell {} at {}",
            i + 1,
            a5::get_resolution(cells[i])
        )));
    }
    Ok(res)
}

fn check_lods(lods: &[i32], data_resolution: i32) -> Result<Vec<i32>> {
    let mut seen = HashSet::new();
    for &l in lods {
        if l < 0 || l >= data_resolution {
            return Err(invalid(format!(
                "LODs must be resolutions from 0 to {}, below the cells' resolution {data_resolution}, not {l}",
                data_resolution - 1
            )));
        }
        if !seen.insert(l) {
            return Err(invalid(format!("LOD {l} is given more than once")));
        }
    }
    Ok(lods.to_vec())
}

/// Cells, value columns, vector columns and weights of the leaves, in one
/// row order.
type LeafParts = (Vec<u64>, Vec<Column>, Vec<VectorColumn>, Option<Vec<f64>>);

/// Leaves in ascending cell order (input already in order is used as is),
/// with duplicates rejected. Sorting needs 16 bytes a leaf for the
/// (cell, row) pairs and 4 for the row order; the cells are rewritten in
/// place and the columns gathered one at a time, so at most one column is
/// held twice (a vector column too: input in cell order avoids the copy).
fn sort_leaves(
    mut cells: Vec<u64>,
    columns: Vec<Column>,
    vectors: Vec<VectorColumn>,
    weights: Option<Vec<f64>>,
) -> Result<LeafParts> {
    if cells.par_windows(2).all(|w| w[0] < w[1]) {
        return Ok((cells, columns, vectors, weights));
    }
    let mut order: Vec<(u64, u32)> = cells.par_iter().enumerate().map(|(i, &c)| (c, i as u32)).collect();
    order.par_sort_unstable();
    if let Some(k) = order.par_windows(2).position_first(|w| w[0].0 == w[1].0) {
        let (cell, a) = order[k];
        let b = order[k + 1].1;
        return Err(invalid(format!(
            "cell {} appears more than once (rows {} and {}); a pyramid needs each cell once",
            crate::hex(cell),
            a.min(b) + 1,
            a.max(b) + 1
        )));
    }
    cells
        .par_iter_mut()
        .zip(order.par_iter())
        .for_each(|(c, &(k, _))| *c = k);
    let rows: Vec<u32> = order.par_iter().map(|&(_, i)| i).collect();
    drop(order);
    let gather = |v: Vec<f64>| -> Vec<f64> { rows.par_iter().map(|&i| v[i as usize]).collect() };
    let columns = columns
        .into_iter()
        .map(|c| Column {
            values: gather(c.values),
            spec: c.spec,
        })
        .collect();
    let weights = weights.map(gather);
    let vectors = vectors
        .into_iter()
        .map(|v| {
            let dim = v.spec.dim;
            let mut out = vec![0u8; v.values.len()];
            out.par_chunks_mut(dim).zip(rows.par_iter()).for_each(|(o, &i)| {
                let i = i as usize;
                o.copy_from_slice(&v.values[i * dim..(i + 1) * dim]);
            });
            VectorColumn {
                spec: v.spec,
                values: out,
            }
        })
        .collect();
    Ok((cells, columns, vectors, weights))
}

/// The `RepChild` columns and the scale (1 / standard deviation, given or
/// over the leaves) that puts each on a common footing in the distance to
/// the mean; every `RepChild` vector column takes part, with one scale for
/// all its elements.
struct RepSpec {
    cols: Vec<usize>,
    sds: Vec<f64>,
    scales: Vec<f64>,
    vecs: Vec<usize>,
    vec_sds: Vec<f64>,
    vec_scales: Vec<f64>,
}

impl RepSpec {
    fn new(columns: &[Column], vectors: &[VectorColumn]) -> Self {
        let cols: Vec<usize> = columns
            .iter()
            .enumerate()
            .filter(|(_, c)| c.spec.reducer == Reducer::RepChild)
            .map(|(k, _)| k)
            .collect();
        let sds: Vec<f64> = cols
            .iter()
            .map(|&k| columns[k].spec.sd.unwrap_or_else(|| leaf_sd(&columns[k].values)))
            .collect();
        let scales = sds.iter().map(|sd| 1.0 / sd).collect();
        let vecs: Vec<usize> = vectors
            .iter()
            .enumerate()
            .filter(|(_, v)| v.spec.reducer == Reducer::RepChild)
            .map(|(k, _)| k)
            .collect();
        let vec_sds: Vec<f64> = vecs
            .iter()
            .map(|&k| {
                let v = &vectors[k];
                v.spec.sd.unwrap_or_else(|| byte_sd(&v.values, v.spec.ty))
            })
            .collect();
        let vec_scales = vec_sds.iter().map(|sd| 1.0 / sd).collect();
        Self {
            cols,
            sds,
            scales,
            vecs,
            vec_sds,
            vec_scales,
        }
    }

    fn is_empty(&self) -> bool {
        self.cols.is_empty() && self.vecs.is_empty()
    }
}

/// The standard deviation of a vector column's elements over all leaves,
/// pooled, or 1 when that is 0 or undefined; in fixed chunks, as
/// [`leaf_sd`], so it does not depend on the thread count.
fn byte_sd(v: &[u8], ty: ColumnType) -> f64 {
    const CHUNK: usize = 1 << 16;
    let sum_chunks = |f: &(dyn Fn(f64) -> f64 + Sync)| -> f64 {
        let parts: Vec<f64> = v
            .par_chunks(CHUNK)
            .map(|c| c.iter().map(|&b| f(ty.byte_value(b))).sum::<f64>())
            .collect();
        parts.iter().sum()
    };
    let n = v.len();
    if n < 2 {
        return 1.0;
    }
    let mean = sum_chunks(&|x| x) / n as f64;
    let ss = sum_chunks(&|x| (x - mean) * (x - mean));
    let sd = (ss / (n - 1) as f64).sqrt();
    if sd > 0.0 && sd.is_finite() {
        sd
    } else {
        1.0
    }
}

/// The standard deviation of the non-missing values, or 1 when that is
/// zero or undefined (a constant column adds nothing to any distance).
/// Sums run over fixed chunks combined in order, so the scale, and with it
/// every choice of representative, is the same on any number of threads.
fn leaf_sd(v: &[f64]) -> f64 {
    const CHUNK: usize = 1 << 16;
    let sum_chunks = |f: &(dyn Fn(f64) -> f64 + Sync)| -> (usize, f64) {
        let parts: Vec<(usize, f64)> = v
            .par_chunks(CHUNK)
            .map(|c| {
                c.iter()
                    .filter(|x| !x.is_nan())
                    .fold((0, 0.0), |(n, s), &x| (n + 1, s + f(x)))
            })
            .collect();
        parts.iter().fold((0, 0.0), |(n, s), &(m, t)| (n + m, s + t))
    };
    let (n, sum) = sum_chunks(&|x| x);
    if n < 2 {
        return 1.0;
    }
    let mean = sum / n as f64;
    let (_, ss) = sum_chunks(&|x| (x - mean) * (x - mean));
    let sd = (ss / (n - 1) as f64).sqrt();
    if sd > 0.0 && sd.is_finite() {
        sd
    } else {
        1.0
    }
}

fn build_level(
    cells: &[u64],
    columns: &[Column],
    vectors: &[VectorColumn],
    weights: Option<&[f64]>,
    rep: &RepSpec,
    lod: i32,
    cancel: &Cancel,
) -> Result<Level> {
    let mut pairs = supercell_rows(cells, lod)?;
    cancel.check()?;
    // (parent, leaf) is a total order, so groups and the leaves within them
    // come out the same whatever the thread count
    pairs.par_sort_unstable();
    cancel.check()?;

    // where each group starts, as u32: a pyramid holds fewer than 2^32 leaves
    let mut starts: Vec<u32> = Vec::new();
    starts.push(0);
    for k in 1..pairs.len() {
        if pairs[k].0 != pairs[k - 1].0 {
            starts.push(k as u32);
        }
    }
    starts.push(pairs.len() as u32);
    let n_groups = starts.len() - 1;
    let group = |g: usize| &pairs[starts[g] as usize..starts[g + 1] as usize];

    let out_cells: Vec<u64> = (0..n_groups)
        .into_par_iter()
        .map(|g| pairs[starts[g] as usize].0)
        .collect();
    let ncell: Vec<u32> = (0..n_groups)
        .into_par_iter()
        .map(|g| starts[g + 1] - starts[g])
        .collect();

    let rep_leaf: Vec<u32> = if rep.is_empty() {
        Vec::new()
    } else {
        let rep_values: Vec<&[f64]> = rep.cols.iter().map(|&k| columns[k].values.as_slice()).collect();
        let views: Vec<VecView> = rep
            .vecs
            .iter()
            .zip(&rep.vec_scales)
            .map(|(&k, &scale)| VecView {
                data: &vectors[k].values,
                dim: vectors[k].spec.dim,
                ty: vectors[k].spec.ty,
                scale,
            })
            .collect();
        let width = rep_values.len() + views.iter().map(|v| v.dim).sum::<usize>();
        (0..n_groups)
            .into_par_iter()
            .map_init(
                || vec![0.0; 2 * width],
                |scratch, g| representative(group(g), &rep_values, &rep.scales, &views, weights, scratch),
            )
            .collect()
    };
    cancel.check()?;

    let mut values = Vec::with_capacity(columns.len());
    for col in columns {
        let v = &col.values;
        let ty = col.spec.ty;
        let out: Vec<f64> = match col.spec.reducer {
            Reducer::RepChild => rep_leaf.par_iter().map(|&i| v[i as usize]).collect(),
            Reducer::Mode => (0..n_groups)
                .into_par_iter()
                .map_init(Vec::new, |scratch, g| mode(group(g), v, weights, scratch))
                .collect(),
            r => {
                let codec = col.spec.codec;
                (0..n_groups)
                    .into_par_iter()
                    .map(|g| ty.quantise(reduce(r, group(g), v, weights, codec)))
                    .collect()
            }
        };
        values.push(out);
        cancel.check()?;
    }

    let mut out_vectors = Vec::with_capacity(vectors.len());
    for v in vectors {
        let dim = v.spec.dim;
        let mut out = vec![0u8; n_groups * dim];
        match v.spec.reducer {
            Reducer::Mean => {
                let lut = value_lut(v.spec.ty, v.spec.codec);
                let mean = VecMean {
                    data: &v.values,
                    dim,
                    ty: v.spec.ty,
                    codec: v.spec.codec,
                    lut: &lut,
                };
                out.par_chunks_mut(dim)
                    .enumerate()
                    .for_each_init(|| vec![0.0; dim], |acc, (g, o)| mean.reduce(group(g), weights, acc, o));
            }
            _ => out.par_chunks_mut(dim).zip(rep_leaf.par_iter()).for_each(|(o, &i)| {
                let i = i as usize;
                o.copy_from_slice(&v.values[i * dim..(i + 1) * dim]);
            }),
        }
        out_vectors.push(out);
        cancel.check()?;
    }

    Ok(Level {
        lod,
        cells: out_cells,
        ncell,
        values,
        vectors: out_vectors,
    })
}

/// (supercell at `res`, row) for every cell, in row order. Filled in place:
/// collecting fallible results would hold the pairs twice while they are
/// joined.
pub(crate) fn supercell_rows(cells: &[u64], res: i32) -> Result<Vec<(u64, u32)>> {
    let mut pairs = vec![(0u64, 0u32); cells.len()];
    pairs
        .par_iter_mut()
        .zip(cells.par_iter())
        .enumerate()
        .try_for_each(|(i, (p, &c))| {
            *p = (a5::cell_to_supercell(c, res)?, i as u32);
            Ok::<(), String>(())
        })
        .map_err(Error::A5)?;
    Ok(pairs)
}

#[inline]
fn weight(weights: Option<&[f64]>, i: usize) -> f64 {
    weights.map_or(1.0, |w| w[i])
}

/// Mean, sum, min or max of the group's non-missing values. A mean with a
/// codec is the code of the mean of the decoded values.
fn reduce(reducer: Reducer, group: &[(u64, u32)], v: &[f64], weights: Option<&[f64]>, codec: Option<Codec>) -> f64 {
    let present = group.iter().map(|&(_, i)| i as usize).filter(|&i| !v[i].is_nan());
    match reducer {
        Reducer::Mean => {
            let (mut s, mut w) = (0.0, 0.0);
            for i in present {
                let wi = weight(weights, i);
                if wi > 0.0 {
                    s += wi * codec.map_or(v[i], |c| c.decode(v[i]));
                    w += wi;
                }
            }
            if w > 0.0 {
                codec.map_or(s / w, |c| c.encode(s / w))
            } else {
                f64::NAN
            }
        }
        Reducer::Sum => present
            .map(|i| v[i])
            .fold(f64::NAN, |a, x| if a.is_nan() { x } else { a + x }),
        Reducer::Min => present
            .map(|i| v[i])
            .fold(f64::NAN, |a, x| if a.is_nan() || x < a { x } else { a }),
        Reducer::Max => present
            .map(|i| v[i])
            .fold(f64::NAN, |a, x| if a.is_nan() || x > a { x } else { a }),
        Reducer::Mode | Reducer::RepChild => unreachable!("handled by the caller"),
    }
}

/// The value of every byte of a vector column of `ty` elements: decoded
/// with a codec, else the element itself.
fn value_lut(ty: ColumnType, codec: Option<Codec>) -> [f64; 256] {
    match codec {
        Some(c) => c.byte_values(ty),
        None => {
            let mut lut = [0.0; 256];
            for (b, v) in lut.iter_mut().enumerate() {
                *v = ty.byte_value(b as u8);
            }
            lut
        }
    }
}

/// A `Mean` vector column as its reduction reads it: the leaves' bytes and
/// the value of each byte.
struct VecMean<'a> {
    data: &'a [u8],
    dim: usize,
    ty: ColumnType,
    codec: Option<Codec>,
    lut: &'a [f64; 256],
}

impl VecMean<'_> {
    /// The group's (weighted) mean vector into `out`, element by element,
    /// stored as the column's type: encoded with a codec, else rounded half
    /// to even and clamped. A vector is never missing, so a group whose
    /// leaves all have weight 0 takes their unweighted mean.
    fn reduce(&self, group: &[(u64, u32)], weights: Option<&[f64]>, acc: &mut [f64], out: &mut [u8]) {
        let dim = self.dim;
        let add = |acc: &mut [f64], i: usize, wi: f64| {
            let row = &self.data[i * dim..(i + 1) * dim];
            for (a, &b) in acc.iter_mut().zip(row) {
                *a += wi * self.lut[b as usize];
            }
        };
        acc.fill(0.0);
        let mut wsum = 0.0;
        for &(_, i) in group {
            let wi = weight(weights, i as usize);
            if wi > 0.0 {
                add(acc, i as usize, wi);
                wsum += wi;
            }
        }
        if wsum == 0.0 {
            for &(_, i) in group {
                add(acc, i as usize, 1.0);
                wsum += 1.0;
            }
        }
        for (o, &a) in out.iter_mut().zip(acc.iter()) {
            let m = a / wsum;
            let q = match self.codec {
                Some(c) => c.encode(m),
                None => self.ty.quantise(m),
            };
            *o = match self.ty {
                ColumnType::Int8 => q as i8 as u8,
                _ => q as u8,
            };
        }
    }
}

/// The group's most frequent value by total weight, ties to the smallest.
fn mode(group: &[(u64, u32)], v: &[f64], weights: Option<&[f64]>, scratch: &mut Vec<(f64, f64)>) -> f64 {
    scratch.clear();
    for &(_, i) in group {
        let i = i as usize;
        let wi = weight(weights, i);
        if !v[i].is_nan() && wi > 0.0 {
            scratch.push((v[i], wi));
        }
    }
    scratch.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
    let (mut best, mut best_w) = (f64::NAN, 0.0);
    let mut k = 0;
    while k < scratch.len() {
        let value = scratch[k].0;
        let mut w = 0.0;
        while k < scratch.len() && scratch[k].0 == value {
            w += scratch[k].1;
            k += 1;
        }
        if w > best_w {
            best = value;
            best_w = w;
        }
    }
    best
}

/// Distances within this relative margin of the best so far count as ties.
/// Ties are common: in a group of two leaves both lie exactly as far from
/// the mean, and rounding alone would decide between them.
const TIE: f64 = 1e-12;

/// A vector column as the representative choice reads it.
struct VecView<'a> {
    data: &'a [u8],
    dim: usize,
    ty: ColumnType,
    scale: f64,
}

/// The leaf of `group` closest to the group's weighted mean over the
/// `RepChild` columns and every element of the vector columns, each scaled
/// to unit standard deviation. Leaves with positive weight come before
/// those without, and leaves with fewer missing values before those with
/// more; ties go to the first leaf, the one with the lowest cell id.
fn representative(
    group: &[(u64, u32)],
    cols: &[&[f64]],
    scales: &[f64],
    vecs: &[VecView],
    weights: Option<&[f64]>,
    scratch: &mut [f64],
) -> u32 {
    let d = cols.len();
    let width = d + vecs.iter().map(|v| v.dim).sum::<usize>();
    let (mean, wsum) = scratch[..2 * width].split_at_mut(width);
    mean.fill(0.0);
    wsum.fill(0.0);
    // vectors are never missing: one weight sum serves all their elements
    let mut wvec = 0.0;
    for &(_, i) in group {
        let i = i as usize;
        let wi = weight(weights, i);
        if wi <= 0.0 {
            continue;
        }
        for k in 0..d {
            let x = cols[k][i];
            if !x.is_nan() {
                mean[k] += wi * x;
                wsum[k] += wi;
            }
        }
        wvec += wi;
        let mut at = d;
        for v in vecs {
            let row = &v.data[i * v.dim..(i + 1) * v.dim];
            for (e, &b) in row.iter().enumerate() {
                mean[at + e] += wi * v.ty.byte_value(b);
            }
            at += v.dim;
        }
    }
    for k in 0..d {
        mean[k] = if wsum[k] > 0.0 { mean[k] / wsum[k] } else { f64::NAN };
    }
    for m in &mut mean[d..] {
        *m = if wvec > 0.0 { *m / wvec } else { f64::NAN };
    }
    let mut best = group[0].1;
    let mut best_key = (u8::MAX, usize::MAX, f64::INFINITY);
    for &(_, i) in group {
        let iu = i as usize;
        let unweighted = u8::from(weight(weights, iu) <= 0.0);
        let mut missing = 0usize;
        let mut dist = 0.0;
        for k in 0..d {
            if mean[k].is_nan() {
                continue;
            }
            let x = cols[k][iu];
            if x.is_nan() {
                missing += 1;
            } else {
                let z = (x - mean[k]) * scales[k];
                dist += z * z;
            }
        }
        if wvec > 0.0 {
            let mut at = d;
            for v in vecs {
                let row = &v.data[iu * v.dim..(iu + 1) * v.dim];
                for (e, &b) in row.iter().enumerate() {
                    let z = (v.ty.byte_value(b) - mean[at + e]) * v.scale;
                    dist += z * z;
                }
                at += v.dim;
            }
        }
        let key = (unweighted, missing, dist);
        let closer = dist < best_key.2 - TIE * best_key.2;
        if key.0 < best_key.0 || (key.0 == best_key.0 && (key.1 < best_key.1 || (key.1 == best_key.1 && closer))) {
            best_key = key;
            best = i;
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::column::ColumnType;
    use crate::testutil::grid;

    fn leaves_with(cells: Vec<u64>, cols: Vec<Column>) -> Leaves {
        Leaves {
            cells,
            columns: cols,
            ..Default::default()
        }
    }

    #[test]
    fn levels_partition_the_leaves() {
        let cells = grid(12, (10.0, 10.6), (50.0, 50.4), 300);
        let n = cells.len();
        let values: Vec<f64> = (0..n).map(|i| i as f64).collect();
        let col = Column::new("v", ColumnType::Float64, Reducer::Sum, values.clone());
        let p = build(leaves_with(cells.clone(), vec![col]), &[6, 8, 10], &Cancel::new()).unwrap();
        assert_eq!(p.lods(), vec![6, 8, 10, 12]);
        let total: f64 = values.iter().sum();
        for level in &p.levels {
            assert_eq!(level.ncell.iter().map(|&c| c as usize).sum::<usize>(), n);
            assert!((level.values[0].iter().sum::<f64>() - total).abs() < 1e-6);
            assert!(level.cells.windows(2).all(|w| w[0] < w[1]));
            assert!(level.cells.iter().all(|&c| a5::get_resolution(c) == level.lod));
        }
        // a LOD cell sums exactly the leaves whose supercell it is
        let l8 = p.level(8).unwrap();
        for (k, &c) in l8.cells.iter().enumerate() {
            let want: f64 = cells
                .iter()
                .zip(&values)
                .filter(|(&leaf, _)| a5::cell_to_supercell(leaf, 8).unwrap() == c)
                .map(|(_, v)| v)
                .sum();
            assert_eq!(l8.values[0][k], want);
        }
    }

    #[test]
    fn reducers_skip_missing_values() {
        let cells = grid(10, (0.0, 0.3), (0.0, 0.3), 100);
        let n = cells.len();
        let mut v: Vec<f64> = (0..n).map(|i| (i % 7) as f64).collect();
        v[0] = f64::NAN;
        let cols = Reducer::ALL
            .iter()
            .map(|&r| Column::new(r.as_str(), ColumnType::Float64, r, v.clone()))
            .collect();
        let p = build(leaves_with(cells.clone(), cols), &[5], &Cancel::new()).unwrap();
        let l5 = &p.levels[0];
        for (k, &c) in l5.cells.iter().enumerate() {
            let xs: Vec<f64> = cells
                .iter()
                .zip(&v)
                .filter(|(&leaf, x)| a5::cell_to_supercell(leaf, 5).unwrap() == c && !x.is_nan())
                .map(|(_, &x)| x)
                .collect();
            let mean = xs.iter().sum::<f64>() / xs.len() as f64;
            assert!((l5.values[0][k] - mean).abs() < 1e-12);
            assert_eq!(l5.values[1][k], xs.iter().sum::<f64>());
            assert_eq!(l5.values[2][k], xs.iter().cloned().fold(f64::INFINITY, f64::min));
            assert_eq!(l5.values[3][k], xs.iter().cloned().fold(f64::NEG_INFINITY, f64::max));
            let mut counts = [0usize; 7];
            xs.iter().for_each(|&x| counts[x as usize] += 1);
            let top = counts.iter().max().unwrap();
            assert_eq!(l5.values[4][k], counts.iter().position(|c| c == top).unwrap() as f64);
            assert!(xs.contains(&l5.values[5][k]));
        }
    }

    #[test]
    fn rep_child_takes_every_column_from_one_leaf() {
        let cells = grid(11, (20.0, 20.4), (-5.0, -4.6), 200);
        let n = cells.len();
        let r: Vec<f64> = (0..n).map(|i| ((i * 37) % 256) as f64).collect();
        let g: Vec<f64> = (0..n).map(|i| ((i * 91) % 256) as f64).collect();
        let cols = vec![
            Column::new("r", ColumnType::UInt8, Reducer::RepChild, r.clone()),
            Column::new("g", ColumnType::UInt8, Reducer::RepChild, g.clone()),
        ];
        let p = build(leaves_with(cells, cols), &[7], &Cancel::new()).unwrap();
        let pairs: HashSet<(u64, u64)> = r.iter().zip(&g).map(|(a, b)| (*a as u64, *b as u64)).collect();
        let l = &p.levels[0];
        for k in 0..l.len() {
            assert!(pairs.contains(&(l.values[0][k] as u64, l.values[1][k] as u64)));
        }
    }

    #[test]
    fn rep_child_records_its_scale_and_a_given_one_is_used() {
        let cells = grid(11, (20.0, 20.4), (-5.0, -4.6), 200);
        let n = cells.len();
        let r: Vec<f64> = (0..n).map(|i| ((i * 37) % 256) as f64).collect();
        let g: Vec<f64> = (0..n).map(|i| ((i * 91) % 256) as f64).collect();
        let mk = |sd: Option<(f64, f64)>| {
            let (mut a, mut b) = (
                Column::new("r", ColumnType::UInt8, Reducer::RepChild, r.clone()),
                Column::new("g", ColumnType::UInt8, Reducer::RepChild, g.clone()),
            );
            if let Some((x, y)) = sd {
                a = a.with_sd(x);
                b = b.with_sd(y);
            }
            let m = Column::new("m", ColumnType::Float64, Reducer::Mean, r.clone());
            build(leaves_with(cells.clone(), vec![a, b, m]), &[7], &Cancel::new()).unwrap()
        };
        let computed = mk(None);
        let sds: Vec<Option<f64>> = computed.columns.iter().map(|c| c.sd).collect();
        assert!(sds[0].unwrap() > 0.0 && sds[1].unwrap() > 0.0 && sds[2].is_none());
        assert_eq!(sds[0], Some(leaf_sd(&r)));
        // the computed scales, given, choose the same leaves
        assert_eq!(mk(Some((sds[0].unwrap(), sds[1].unwrap()))), computed);
        // a scale that all but drops g picks the leaf closest in r alone
        let lopsided = mk(Some((1.0, 1e9)));
        assert_eq!(lopsided.columns[1].sd, Some(1e9));
        assert_ne!(lopsided.levels[0].values, computed.levels[0].values);
    }

    #[test]
    fn a_fixed_scale_makes_a_cell_depend_on_its_own_leaves_only() {
        // the same region built whole and with a part of it left out: with
        // the scale fixed, every coarse cell whose leaves are all in the
        // part picks the same representative
        let cells = grid(11, (20.0, 20.6), (-5.0, -4.6), 260);
        let n = cells.len();
        let v: Vec<f64> = (0..n).map(|i| (((i * 2654435761) % 1000) as f64).sqrt()).collect();
        let w: Vec<f64> = (0..n).map(|i| ((i * 40503) % 977) as f64).collect();
        let part: Vec<usize> = (0..n)
            .filter(|&i| a5::cell_to_lonlat(cells[i]).unwrap().longitude() < 20.3)
            .collect();
        let mk = |rows: &[usize], sd: Option<f64>| {
            let col = |name: &str, x: &[f64]| {
                let c = Column::new(
                    name,
                    ColumnType::Float64,
                    Reducer::RepChild,
                    rows.iter().map(|&i| x[i]).collect(),
                );
                sd.map_or(c.clone(), |s| c.with_sd(s))
            };
            let cells = rows.iter().map(|&i| cells[i]).collect();
            build(
                leaves_with(cells, vec![col("v", &v), col("w", &w)]),
                &[6],
                &Cancel::new(),
            )
            .unwrap()
        };
        let all: Vec<usize> = (0..n).collect();
        let same = |whole: &Pyramid, sub: &Pyramid| {
            let (a, b) = (&whole.levels[0], &sub.levels[0]);
            let mut agree = 0;
            for (k, &c) in b.cells.iter().enumerate() {
                let j = a.cells.binary_search(&c).unwrap();
                if a.ncell[j] == b.ncell[k] {
                    assert_eq!((a.values[0][j], a.values[1][j]), (b.values[0][k], b.values[1][k]));
                    agree += 1;
                }
            }
            agree
        };
        assert!(same(&mk(&all, Some(10.0)), &mk(&part, Some(10.0))) > 0);
    }

    /// Leaves with a 4-element int8 vector per cell, and the same elements
    /// as four scalar int8 columns.
    fn vector_leaves(cells: &[u64]) -> (Vec<u8>, Vec<Vec<f64>>) {
        let n = cells.len();
        let dim = 4;
        let mut bytes = vec![0u8; n * dim];
        let mut cols = vec![vec![0.0; n]; dim];
        for i in 0..n {
            for e in 0..dim {
                let x = (((i * (7 + 13 * e)) ^ (i >> 3)) % 251) as i32 - 125;
                bytes[i * dim + e] = (x as i8) as u8;
                cols[e][i] = x as f64;
            }
        }
        (bytes, cols)
    }

    #[test]
    fn a_vector_chooses_as_its_elements_would_as_columns_of_one_scale() {
        let cells = grid(11, (20.0, 20.4), (-5.0, -4.6), 200);
        let (bytes, cols) = vector_leaves(&cells);
        let sd = 40.0;
        let as_vector = build(
            Leaves {
                cells: cells.clone(),
                vectors: vec![VectorColumn::new("e", ColumnType::Int8, 4, bytes).with_sd(sd)],
                ..Default::default()
            },
            &[6, 8],
            &Cancel::new(),
        )
        .unwrap();
        let columns = cols
            .iter()
            .enumerate()
            .map(|(e, v)| Column::new(format!("e{e}"), ColumnType::Int8, Reducer::RepChild, v.clone()).with_sd(sd))
            .collect();
        let as_columns = build(leaves_with(cells, columns), &[6, 8], &Cancel::new()).unwrap();
        assert_eq!(as_vector.vectors[0].sd, Some(sd));
        for (a, b) in as_vector.levels.iter().zip(&as_columns.levels) {
            assert_eq!(a.cells, b.cells);
            for i in 0..a.len() {
                for e in 0..4 {
                    assert_eq!(a.vectors[0][i * 4 + e] as i8 as f64, b.values[e][i]);
                }
            }
        }
    }

    #[test]
    fn vectors_and_rep_child_columns_share_one_leaf() {
        let cells = grid(11, (20.0, 20.4), (-5.0, -4.6), 200);
        let n = cells.len();
        let (bytes, _) = vector_leaves(&cells);
        let r: Vec<f64> = (0..n).map(|i| ((i * 37) % 256) as f64).collect();
        let leaves = Leaves {
            cells: cells.clone(),
            columns: vec![Column::new("r", ColumnType::UInt8, Reducer::RepChild, r.clone())],
            vectors: vec![VectorColumn::new("e", ColumnType::UInt8, 4, bytes.clone())],
            ..Default::default()
        };
        let p = build(leaves, &[7], &Cancel::new()).unwrap();
        // computed sd: over every element of every leaf
        let all: Vec<f64> = bytes.iter().map(|&b| b as f64).collect();
        assert!((p.vectors[0].sd.unwrap() - leaf_sd(&all)).abs() < 1e-9);
        let rows: HashSet<(u64, Vec<u8>)> = (0..n)
            .map(|i| (r[i] as u64, bytes[i * 4..i * 4 + 4].to_vec()))
            .collect();
        let l = &p.levels[0];
        for i in 0..l.len() {
            let got = (l.values[0][i] as u64, l.vectors[0][i * 4..i * 4 + 4].to_vec());
            assert!(rows.contains(&got), "cell {i} mixes leaves");
        }
        // the leaves keep their vectors in cell order
        assert_eq!(p.levels.last().unwrap().vectors[0], bytes);
    }

    #[test]
    fn unsorted_vectors_follow_their_cells() {
        let cells = grid(10, (5.0, 5.4), (45.0, 45.3), 120);
        let n = cells.len();
        let (bytes, _) = vector_leaves(&cells);
        let mk = |order: &[usize]| {
            let c = order.iter().map(|&i| cells[i]).collect();
            let b = order.iter().flat_map(|&i| bytes[i * 4..i * 4 + 4].to_vec()).collect();
            let leaves = Leaves {
                cells: c,
                vectors: vec![VectorColumn::new("e", ColumnType::Int8, 4, b)],
                ..Default::default()
            };
            build(leaves, &[4, 6, 8], &Cancel::new()).unwrap()
        };
        let sorted: Vec<usize> = (0..n).collect();
        let reversed: Vec<usize> = (0..n).rev().collect();
        assert_eq!(mk(&sorted), mk(&reversed));
    }

    #[test]
    fn bad_vectors_are_refused() {
        let cells = grid(8, (0.0, 1.0), (0.0, 1.0), 30);
        let n = cells.len();
        let try_build = |v: VectorColumn, cols: Vec<Column>| {
            build(
                Leaves {
                    cells: cells.clone(),
                    columns: cols,
                    vectors: vec![v],
                    ..Default::default()
                },
                &[],
                &Cancel::new(),
            )
        };
        let ok = |dim: usize| vec![0u8; n * dim];
        assert!(try_build(VectorColumn::new("e", ColumnType::Int8, 4, ok(4)), vec![]).is_ok());
        assert!(try_build(VectorColumn::new("e", ColumnType::Int8, 4, ok(3)), vec![]).is_err());
        assert!(try_build(VectorColumn::new("e", ColumnType::Int8, 0, vec![]), vec![]).is_err());
        assert!(try_build(VectorColumn::new("e", ColumnType::Int16, 4, ok(4)), vec![]).is_err());
        assert!(try_build(VectorColumn::new("ncell", ColumnType::Int8, 4, ok(4)), vec![]).is_err());
        assert!(try_build(VectorColumn::new("e", ColumnType::Int8, 4, ok(4)).with_sd(0.0), vec![]).is_err());
        let clash = Column::new("e", ColumnType::Float64, Reducer::Mean, vec![1.0; n]);
        assert!(try_build(VectorColumn::new("e", ColumnType::Int8, 4, ok(4)), vec![clash]).is_err());
    }

    /// The code of the weighted mean of the decoded codes of the leaves under
    /// `parent` at `lod`, computed the long way, with the leaves in cell
    /// order (the order the engine sums them in).
    fn expected_aef_mean(cells: &[u64], codes: &[f64], w: &[f64], parent: u64, lod: i32) -> f64 {
        let aef = Codec::Aef;
        let mut idx: Vec<usize> = (0..cells.len())
            .filter(|&i| a5::cell_to_supercell(cells[i], lod).unwrap() == parent)
            .collect();
        idx.sort_by_key(|&i| cells[i]);
        let (mut s, mut ws) = (0.0, 0.0);
        for i in idx {
            if w[i] > 0.0 {
                s += w[i] * aef.decode(codes[i]);
                ws += w[i];
            }
        }
        aef.encode(s / ws)
    }

    #[test]
    fn a_codec_mean_is_the_code_of_the_mean_of_the_decoded_leaves() {
        let cells = grid(11, (20.0, 20.4), (-5.0, -4.6), 200);
        let n = cells.len();
        let (_, cols) = vector_leaves(&cells);
        let codes = cols[1].clone();
        let w: Vec<f64> = (0..n).map(|i| 1.0 + (i % 5) as f64).collect();
        let leaves = Leaves {
            cells: cells.clone(),
            columns: vec![
                Column::new("aef", ColumnType::Int8, Reducer::Mean, codes.clone()).with_codec(Codec::Aef),
                Column::new("raw", ColumnType::Int8, Reducer::Mean, codes.clone()),
            ],
            weights: Some(w.clone()),
            ..Default::default()
        };
        let p = build(leaves, &[7, 9], &Cancel::new()).unwrap();
        assert_eq!(p.columns[0].codec, Some(Codec::Aef));
        assert_eq!(p.columns[1].codec, None);
        let mut differs = 0;
        for level in &p.levels[..2] {
            for (k, &c) in level.cells.iter().enumerate() {
                assert_eq!(level.values[0][k], expected_aef_mean(&cells, &codes, &w, c, level.lod));
                differs += usize::from(level.values[0][k] != level.values[1][k]);
            }
        }
        // the mean of the codes is not the code of the mean
        assert!(differs > 0);
        // the leaves keep their codes
        assert_eq!(p.levels[2].values[0], codes);
    }

    #[test]
    fn a_mean_vector_is_its_elements_means() {
        let cells = grid(11, (20.0, 20.4), (-5.0, -4.6), 200);
        let n = cells.len();
        let (bytes, cols) = vector_leaves(&cells);
        let w: Vec<f64> = (0..n).map(|i| (i % 4) as f64).collect();
        for codec in [None, Some(Codec::Aef)] {
            let mut v = VectorColumn::new("e", ColumnType::Int8, 4, bytes.clone()).with_reducer(Reducer::Mean);
            if let Some(c) = codec {
                v = v.with_codec(c);
            }
            let as_vector = build(
                Leaves {
                    cells: cells.clone(),
                    vectors: vec![v],
                    weights: Some(w.clone()),
                    ..Default::default()
                },
                &[6, 8, 10],
                &Cancel::new(),
            )
            .unwrap();
            let columns = cols
                .iter()
                .enumerate()
                .map(|(e, x)| {
                    let c = Column::new(format!("e{e}"), ColumnType::Int8, Reducer::Mean, x.clone());
                    match codec {
                        Some(k) => c.with_codec(k),
                        None => c,
                    }
                })
                .collect();
            let as_columns = build(
                Leaves {
                    cells: cells.clone(),
                    columns,
                    weights: Some(w.clone()),
                    ..Default::default()
                },
                &[6, 8, 10],
                &Cancel::new(),
            )
            .unwrap();
            let spec = &as_vector.vectors[0];
            assert_eq!((spec.reducer, spec.codec, spec.sd), (Reducer::Mean, codec, None));
            for (a, b) in as_vector.levels.iter().zip(&as_columns.levels) {
                assert_eq!(a.cells, b.cells);
                for i in 0..a.len() {
                    for e in 0..4 {
                        // a cell whose leaves all weigh 0 is missing as a
                        // scalar; a vector takes the unweighted mean
                        if !b.values[e][i].is_nan() {
                            assert_eq!(a.vectors[0][i * 4 + e] as i8 as f64, b.values[e][i], "{codec:?}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_vector_whose_leaves_all_weigh_nothing_takes_their_plain_mean() {
        let cells = grid(9, (0.0, 0.5), (0.0, 0.5), 60);
        let n = cells.len();
        let (bytes, cols) = vector_leaves(&cells);
        let mk = |w: Option<Vec<f64>>| {
            build(
                Leaves {
                    cells: cells.clone(),
                    vectors: vec![VectorColumn::new("e", ColumnType::Int8, 4, bytes.clone())
                        .with_reducer(Reducer::Mean)
                        .with_codec(Codec::Aef)],
                    weights: w,
                    ..Default::default()
                },
                &[5],
                &Cancel::new(),
            )
            .unwrap()
        };
        assert_eq!(mk(Some(vec![0.0; n])), mk(None));
        let l = &mk(None).levels[0];
        let ones = vec![1.0; n];
        for (k, &c) in l.cells.iter().enumerate() {
            for (e, col) in cols.iter().enumerate() {
                assert_eq!(
                    l.vectors[0][k * 4 + e] as i8 as f64,
                    expected_aef_mean(&cells, col, &ones, c, 5)
                );
            }
        }
    }

    #[test]
    fn a_mean_vector_leaves_the_representatives_alone() {
        let cells = grid(11, (20.0, 20.4), (-5.0, -4.6), 200);
        let n = cells.len();
        let (bytes, _) = vector_leaves(&cells);
        let r: Vec<f64> = (0..n).map(|i| ((i * 37) % 256) as f64).collect();
        let mk = |with_mean: bool| {
            let mut vectors = vec![VectorColumn::new("e", ColumnType::Int8, 4, bytes.clone())];
            if with_mean {
                vectors.push(
                    VectorColumn::new("m", ColumnType::Int8, 4, bytes.iter().rev().cloned().collect())
                        .with_reducer(Reducer::Mean)
                        .with_codec(Codec::Aef),
                );
            }
            build(
                Leaves {
                    cells: cells.clone(),
                    columns: vec![Column::new("r", ColumnType::UInt8, Reducer::RepChild, r.clone())],
                    vectors,
                    ..Default::default()
                },
                &[7, 9],
                &Cancel::new(),
            )
            .unwrap()
        };
        let (with, without) = (mk(true), mk(false));
        assert_eq!(with.vectors[0].sd, without.vectors[0].sd);
        assert_eq!(with.vectors[1].sd, None);
        for (a, b) in with.levels.iter().zip(&without.levels) {
            assert_eq!(a.values, b.values);
            assert_eq!(a.vectors[0], b.vectors[0]);
        }
    }

    #[test]
    fn codecs_and_vector_reducers_are_checked() {
        let cells = grid(8, (0.0, 1.0), (0.0, 1.0), 30);
        let n = cells.len();
        let try_build = |cols: Vec<Column>, vecs: Vec<VectorColumn>| {
            build(
                Leaves {
                    cells: cells.clone(),
                    columns: cols,
                    vectors: vecs,
                    ..Default::default()
                },
                &[3],
                &Cancel::new(),
            )
        };
        let col = |ty, r| Column::new("v", ty, r, vec![1.0; n]);
        let vec = || VectorColumn::new("e", ColumnType::Int8, 4, vec![0u8; n * 4]);
        assert!(try_build(
            vec![col(ColumnType::Int8, Reducer::Mean).with_codec(Codec::Aef)],
            vec![]
        )
        .is_ok());
        assert!(try_build(
            vec![col(ColumnType::Int16, Reducer::Mean).with_codec(Codec::Aef)],
            vec![]
        )
        .is_err());
        assert!(try_build(
            vec![col(ColumnType::Float64, Reducer::Mean).with_codec(Codec::Aef)],
            vec![]
        )
        .is_err());
        for r in [
            Reducer::Sum,
            Reducer::Min,
            Reducer::Max,
            Reducer::Mode,
            Reducer::RepChild,
        ] {
            assert!(try_build(vec![col(ColumnType::Int8, r).with_codec(Codec::Aef)], vec![]).is_err());
        }
        assert!(try_build(vec![], vec![vec().with_reducer(Reducer::Mean)]).is_ok());
        assert!(try_build(vec![], vec![vec().with_reducer(Reducer::Mean).with_codec(Codec::Aef)]).is_ok());
        for r in [Reducer::Sum, Reducer::Min, Reducer::Max, Reducer::Mode] {
            assert!(try_build(vec![], vec![vec().with_reducer(r)]).is_err());
        }
        assert!(try_build(vec![], vec![vec().with_codec(Codec::Aef)]).is_err());
        assert!(try_build(vec![], vec![vec().with_reducer(Reducer::Mean).with_sd(2.0)]).is_err());
        let uint = VectorColumn::new("e", ColumnType::UInt8, 4, vec![0u8; n * 4]);
        assert!(try_build(vec![], vec![uint.with_reducer(Reducer::Mean).with_codec(Codec::Aef)]).is_err());
    }

    #[test]
    fn ties_go_to_the_lowest_cell() {
        // two leaves per group, equally far from their mean
        let cells = grid(10, (3.0, 3.4), (40.0, 40.3), 120);
        let n = cells.len();
        let v: Vec<f64> = (0..n).map(|i| 0.1 + (i % 2) as f64 * 0.7).collect();
        let col = Column::new("v", ColumnType::Float64, Reducer::RepChild, v.clone());
        let p = build(leaves_with(cells.clone(), vec![col]), &[9], &Cancel::new()).unwrap();
        let l = &p.levels[0];
        for (k, &c) in l.cells.iter().enumerate() {
            let first = cells
                .iter()
                .position(|&leaf| a5::cell_to_supercell(leaf, 9).unwrap() == c)
                .unwrap();
            if l.ncell[k] == 2 {
                assert_eq!(l.values[0][k], v[first]);
            }
        }
    }

    #[test]
    fn weights_shift_means() {
        let cells = grid(9, (0.0, 0.5), (0.0, 0.5), 60);
        let n = cells.len();
        let v: Vec<f64> = (0..n).map(|i| (i % 2) as f64).collect();
        let w: Vec<f64> = (0..n).map(|i| if i % 2 == 0 { 0.0 } else { 3.0 }).collect();
        let col = Column::new("v", ColumnType::Float64, Reducer::Mean, v);
        let leaves = Leaves {
            cells,
            columns: vec![col],
            weights: Some(w),
            ..Default::default()
        };
        let p = build(leaves, &[4], &Cancel::new()).unwrap();
        // only odd leaves (value 1) carry weight
        assert!(p.levels[0].values[0].iter().all(|&x| x == 1.0));
    }

    #[test]
    fn integer_means_round_to_the_column_type() {
        let cells = grid(8, (0.0, 1.0), (0.0, 1.0), 50);
        let n = cells.len();
        let v: Vec<f64> = (0..n).map(|i| (i % 2) as f64 * 255.0).collect();
        let col = Column::new("a", ColumnType::UInt8, Reducer::Mean, v);
        let p = build(leaves_with(cells, vec![col]), &[3], &Cancel::new()).unwrap();
        assert!(p.levels[0].values[0]
            .iter()
            .all(|x| x.fract() == 0.0 && (0.0..=255.0).contains(x)));
    }

    #[test]
    fn unsorted_input_gives_the_same_pyramid() {
        let cells = grid(10, (5.0, 5.4), (45.0, 45.3), 120);
        let n = cells.len();
        let v: Vec<f64> = (0..n).map(|i| (i as f64).sin()).collect();
        let mk = |cells: Vec<u64>, v: Vec<f64>| {
            let col = Column::new("v", ColumnType::Float64, Reducer::RepChild, v);
            build(leaves_with(cells, vec![col]), &[4, 6, 8], &Cancel::new()).unwrap()
        };
        let sorted = mk(cells.clone(), v.clone());
        let order: Vec<usize> = (0..n).rev().collect();
        let shuffled = mk(
            order.iter().map(|&i| cells[i]).collect(),
            order.iter().map(|&i| v[i]).collect(),
        );
        assert_eq!(sorted, shuffled);
    }

    #[test]
    fn same_result_on_any_thread_count() {
        let cells = grid(12, (-70.8, -70.3), (-33.6, -33.2), 400);
        let n = cells.len();
        let v: Vec<f64> = (0..n).map(|i| ((i * 2654435761) % 1000) as f64).collect();
        let run = |threads| {
            let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
            pool.install(|| {
                let cols = vec![
                    Column::new("m", ColumnType::Float64, Reducer::Mean, v.clone()),
                    Column::new("r", ColumnType::Float64, Reducer::RepChild, v.clone()),
                ];
                build(leaves_with(cells.clone(), cols), &default_lods(12), &Cancel::new()).unwrap()
            })
        };
        assert_eq!(run(1), run(8));
    }

    #[test]
    fn bad_input_is_rejected() {
        let cells = grid(6, (0.0, 2.0), (0.0, 2.0), 20);
        let cancel = Cancel::new();
        let dup = vec![cells[0], cells[1], cells[0]];
        assert!(matches!(
            build(leaves_with(dup, vec![]), &[], &cancel),
            Err(Error::Invalid(_))
        ));
        let mut mixed = cells.clone();
        mixed.push(a5::cell_to_parent(cells[0], Some(5)).unwrap());
        assert!(build(leaves_with(mixed, vec![]), &[], &cancel).is_err());
        assert!(build(leaves_with(vec![a5::WORLD_CELL], vec![]), &[], &cancel).is_err());
        assert!(build(leaves_with(cells.clone(), vec![]), &[6], &cancel).is_err());
        assert!(build(leaves_with(cells.clone(), vec![]), &[3, 3], &cancel).is_err());
        let short = Column::new("v", ColumnType::Float64, Reducer::Mean, vec![1.0]);
        assert!(build(leaves_with(cells.clone(), vec![short]), &[], &cancel).is_err());
        let reserved = Column::new("lod", ColumnType::Float64, Reducer::Mean, vec![1.0; cells.len()]);
        assert!(build(leaves_with(cells.clone(), vec![reserved]), &[], &cancel).is_err());
        for (reducer, sd) in [
            (Reducer::Mean, 1.0),
            (Reducer::RepChild, 0.0),
            (Reducer::RepChild, f64::NAN),
        ] {
            let col = Column::new("v", ColumnType::Float64, reducer, vec![1.0; cells.len()]).with_sd(sd);
            assert!(build(leaves_with(cells.clone(), vec![col]), &[], &cancel).is_err());
        }
        let stop = Cancel::new();
        stop.cancel();
        assert!(matches!(
            build(leaves_with(cells, vec![]), &[2], &stop),
            Err(Error::Cancelled)
        ));
    }
}
