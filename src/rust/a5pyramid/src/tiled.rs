//! The tiled form of a pyramid, in Arrow.
//!
//! Each level is split into tiles of nearby cells ([`crate::plan_tiles`]),
//! levels in ascending LOD order, and every tile is one Arrow record batch.
//! The schema metadata holds the index ([`PyramidMeta`], under
//! [`META_KEY`]), so the batches are the whole pyramid: what a renderer
//! needs, in memory, without a file. [`crate::write_parquet`] writes each
//! batch as one row group, so batch `i` of a [`TiledPyramid`] and row group
//! `i` of its file hold the same rows, tile `i` of the one index that
//! describes both.
//!
//! [`TiledPyramid::into_reader`] streams the batches through an
//! [`arrow_array::RecordBatchReader`], building each when it is read, which
//! is the form other Arrow libraries take: an IPC writer, a query engine,
//! or the Arrow C stream interface to R and Python.

use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::sync::Arc;

use arrow_array::{
    ArrayRef, FixedSizeBinaryArray, Float32Array, Float64Array, Int16Array, Int32Array, Int8Array, RecordBatch,
    RecordBatchReader, UInt16Array, UInt32Array, UInt64Array, UInt8Array,
};
use arrow_buffer::{Buffer, NullBuffer};
use arrow_schema::{ArrowError, DataType, Field, Schema, SchemaRef};
use rayon::prelude::*;

use crate::bbox::{cells_bbox, Bbox};
use crate::build::Pyramid;
use crate::cancel::Cancel;
use crate::column::{ColumnSpec, ColumnType, VectorSpec};
use crate::error::{invalid, Error, Result};
use crate::meta::{
    column_meta, version_with, LevelMeta, PyramidMeta, ShardMeta, A5_INDEX, A5_INDEX_KEY, META_KEY, SHARD_FILE_VERSION,
    SINGLE_FILE_VERSION,
};
use crate::tiles::plan_tiles;
use crate::{CELL_COLUMN, LOD_COLUMN, NCELL_COLUMN};

/// Metadata keys a caller may not set.
const RESERVED_KEYS: [&str; 3] = [META_KEY, A5_INDEX_KEY, "ARROW:schema"];

#[derive(Clone, Debug)]
pub struct TileOptions {
    /// The most rows in a tile.
    pub tile_size: usize,
    /// Recorded as the index's `generator`.
    pub generator: String,
    /// Extra metadata, such as a renderer's settings, stored beside the index:
    /// in the schema metadata, and in a Parquet file's key-value metadata.
    /// Keys must be distinct, and may not be [`META_KEY`], [`A5_INDEX_KEY`]
    /// or `ARROW:schema`.
    pub metadata: Vec<(String, String)>,
    /// Set when the pyramid is one shard of a pyramid set
    /// ([`crate::write_set`]): the index records it, takes format version 2,
    /// and boxes every tile rather than only the finest level's.
    pub shard: Option<ShardMeta>,
}

impl Default for TileOptions {
    fn default() -> Self {
        Self {
            tile_size: 4096,
            generator: format!("a5pyramid {}", env!("CARGO_PKG_VERSION")),
            metadata: Vec::new(),
            shard: None,
        }
    }
}

/// A pyramid split into tiles, each an Arrow record batch, with its index.
#[derive(Clone, Debug)]
pub struct TiledPyramid {
    pyramid: Pyramid,
    tiles: Vec<Tile>,
    meta: PyramidMeta,
    metadata: Vec<(String, String)>,
    schema: SchemaRef,
}

/// A tile: rows `rows` of level `level` of the pyramid.
#[derive(Clone, Debug)]
struct Tile {
    level: usize,
    rows: Range<usize>,
}

impl TiledPyramid {
    /// Tile every level of `pyramid` and index the tiles. Bounding boxes come
    /// from the cells' boundaries and are computed in parallel.
    pub fn new(pyramid: Pyramid, opts: &TileOptions, cancel: &Cancel) -> Result<Self> {
        if opts.tile_size == 0 {
            return Err(invalid("tile_size must be at least 1"));
        }
        let mut seen = HashSet::new();
        for (k, _) in &opts.metadata {
            if RESERVED_KEYS.contains(&k.as_str()) {
                return Err(invalid(format!("metadata key {k:?} is reserved")));
            }
            if !seen.insert(k.as_str()) {
                return Err(invalid(format!("metadata key {k:?} is given more than once")));
            }
        }

        if opts.shard.is_some() && pyramid.n_rows() == 0 {
            return Err(invalid("a shard must hold at least one cell"));
        }

        let mut tiles = Vec::new();
        let mut levels = Vec::with_capacity(pyramid.levels.len());
        let mut tile_bbox = Vec::new();
        let mut leaf_boxes = Vec::new();
        let mut all_boxes = Vec::new();
        for (k, level) in pyramid.levels.iter().enumerate() {
            cancel.check()?;
            let ranges = plan_tiles(&level.cells, opts.tile_size)?;
            let boxes = ranges
                .par_iter()
                .map(|r| cells_bbox(&level.cells[r.clone()]))
                .collect::<Result<Vec<Bbox>>>()?;
            levels.push(LevelMeta {
                lod: level.lod,
                rows: level.len() as u64,
                tiles: [tiles.len(), tiles.len() + ranges.len()],
            });
            tile_bbox.extend(boxes.iter().flat_map(|b| b.to_array()));
            tiles.extend(ranges.into_iter().map(|rows| Tile { level: k, rows }));
            all_boxes.extend(boxes.iter().copied());
            leaf_boxes = boxes;
        }
        // A single file's box is its leaves'. A shard's must hold every
        // tile, since coarse cells reach past the leaves under them, and a
        // shard's finest level can be empty; it holds a cell, so some tile
        // has a box.
        let (format_version, boxed) = match opts.shard {
            None => (SINGLE_FILE_VERSION, &leaf_boxes),
            Some(_) => (SHARD_FILE_VERSION, &all_boxes),
        };
        let meta = PyramidMeta {
            format_version: version_with(&pyramid.vectors, format_version),
            generator: opts.generator.clone(),
            a5_crate: crate::A5_CRATE_VERSION.to_string(),
            data_resolution: pyramid.data_resolution,
            tile_size: opts.tile_size,
            bbox: Bbox::covering(boxed).unwrap_or(Bbox::WORLD).to_array(),
            columns: column_meta(&pyramid.columns, &pyramid.vectors),
            levels,
            tile_bbox,
            shard: opts.shard.clone(),
        };

        let mut metadata = vec![
            (META_KEY.to_string(), meta.to_json()?),
            (A5_INDEX_KEY.to_string(), A5_INDEX.to_string()),
        ];
        metadata.extend(opts.metadata.iter().cloned());
        let map: HashMap<String, String> = metadata.iter().cloned().collect();
        let schema = Arc::new(Schema::new(fields(&pyramid.columns, &pyramid.vectors)).with_metadata(map));
        Ok(Self {
            pyramid,
            tiles,
            meta,
            metadata,
            schema,
        })
    }

    /// The index: levels, columns and every tile's bounding box.
    pub fn meta(&self) -> &PyramidMeta {
        &self.meta
    }

    /// The schema of every batch, with the index under [`META_KEY`], the A5
    /// index version under [`A5_INDEX_KEY`] and the caller's metadata in its
    /// metadata.
    pub fn schema(&self) -> SchemaRef {
        Arc::clone(&self.schema)
    }

    /// The same metadata as the schema's, in order: the index, the A5 index
    /// version, then the caller's.
    pub fn metadata(&self) -> &[(String, String)] {
        &self.metadata
    }

    pub fn pyramid(&self) -> &Pyramid {
        &self.pyramid
    }

    pub fn into_pyramid(self) -> Pyramid {
        self.pyramid
    }

    pub fn n_tiles(&self) -> usize {
        self.tiles.len()
    }

    /// Rows in each tile.
    pub fn tile_rows(&self) -> Vec<usize> {
        self.tiles.iter().map(|t| t.rows.len()).collect()
    }

    /// Tile `i` as a record batch. Each call builds the batch from the
    /// pyramid's levels.
    pub fn tile(&self, i: usize) -> Result<RecordBatch> {
        let tile = self
            .tiles
            .get(i)
            .ok_or_else(|| invalid(format!("tile {i} does not exist; the pyramid has {}", self.tiles.len())))?;
        let level = &self.pyramid.levels[tile.level];
        let r = tile.rows.clone();
        let lod = u8::try_from(level.lod).map_err(|_| invalid(format!("LOD {} out of range", level.lod)))?;
        let mut arrays: Vec<ArrayRef> = vec![
            Arc::new(UInt64Array::from(level.cells[r.clone()].to_vec())),
            Arc::new(UInt8Array::from(vec![lod; r.len()])),
            Arc::new(UInt32Array::from(level.ncell[r.clone()].to_vec())),
        ];
        arrays.extend(
            self.pyramid
                .columns
                .iter()
                .zip(&level.values)
                .map(|(c, v)| value_array(c.ty, &v[r.clone()])),
        );
        for (spec, v) in self.pyramid.vectors.iter().zip(&level.vectors) {
            let bytes = &v[r.start * spec.dim..r.end * spec.dim];
            arrays.push(Arc::new(FixedSizeBinaryArray::try_new(
                spec.dim as i32,
                Buffer::from(bytes.to_vec()),
                None,
            )?));
        }
        Ok(RecordBatch::try_new(Arc::clone(&self.schema), arrays)?)
    }

    /// Every tile as a record batch, in order.
    pub fn batches(&self) -> impl Iterator<Item = Result<RecordBatch>> + '_ {
        (0..self.tiles.len()).map(|i| self.tile(i))
    }

    /// The tiles at `lods` (all levels when `None`) whose boxes meet `bbox`
    /// (everywhere when `None`), in order; see [`PyramidMeta::select`].
    pub fn select(&self, lods: Option<&[i32]>, bbox: Option<&Bbox>) -> Result<Vec<usize>> {
        self.meta.select(lods, bbox)
    }

    /// A reader of every tile in order, which owns the pyramid and builds each
    /// batch when it is read.
    pub fn into_reader(self) -> TileReader {
        TileReader { tiled: self, next: 0 }
    }
}

/// Streams the tiles of a [`TiledPyramid`] as record batches.
#[derive(Debug)]
pub struct TileReader {
    tiled: TiledPyramid,
    next: usize,
}

impl TileReader {
    pub fn meta(&self) -> &PyramidMeta {
        self.tiled.meta()
    }
}

impl Iterator for TileReader {
    type Item = std::result::Result<RecordBatch, ArrowError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next >= self.tiled.n_tiles() {
            return None;
        }
        let batch = self.tiled.tile(self.next).map_err(|e| match e {
            Error::Arrow(e) => e,
            e => ArrowError::ExternalError(Box::new(e)),
        });
        self.next += 1;
        Some(batch)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let n = self.tiled.n_tiles() - self.next;
        (n, Some(n))
    }
}

impl RecordBatchReader for TileReader {
    fn schema(&self) -> SchemaRef {
        self.tiled.schema()
    }
}

/// The columns: `cell`, `lod`, `ncell`, the value columns (nullable), then
/// the vector columns as fixed-length binary of `dim` bytes.
fn fields(columns: &[ColumnSpec], vectors: &[VectorSpec]) -> Vec<Field> {
    let mut fields = vec![
        Field::new(CELL_COLUMN, DataType::UInt64, false),
        Field::new(LOD_COLUMN, DataType::UInt8, false),
        Field::new(NCELL_COLUMN, DataType::UInt32, false),
    ];
    fields.extend(columns.iter().map(|c| Field::new(&c.name, c.ty.data_type(), true)));
    fields.extend(
        vectors
            .iter()
            .map(|v| Field::new(&v.name, DataType::FixedSizeBinary(v.dim as i32), false)),
    );
    fields
}

/// Values (NaN where missing) as an Arrow array of `ty`, nulls where
/// missing. Values are already quantised to `ty` by the build.
fn value_array(ty: ColumnType, v: &[f64]) -> ArrayRef {
    let nulls = v
        .iter()
        .any(|x| x.is_nan())
        .then(|| NullBuffer::from(v.iter().map(|x| !x.is_nan()).collect::<Vec<bool>>()));
    macro_rules! typed {
        ($array:ty, $t:ty) => {
            Arc::new(<$array>::new(
                v.iter()
                    .map(|&x| if x.is_nan() { <$t>::default() } else { x as $t })
                    .collect::<Vec<$t>>()
                    .into(),
                nulls,
            ))
        };
    }
    match ty {
        ColumnType::Int8 => typed!(Int8Array, i8),
        ColumnType::Int16 => typed!(Int16Array, i16),
        ColumnType::Int32 => typed!(Int32Array, i32),
        ColumnType::UInt8 => typed!(UInt8Array, u8),
        ColumnType::UInt16 => typed!(UInt16Array, u16),
        ColumnType::UInt32 => typed!(UInt32Array, u32),
        ColumnType::Float32 => typed!(Float32Array, f32),
        ColumnType::Float64 => typed!(Float64Array, f64),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::grid;
    use crate::{build, default_lods, write_parquet, Column, Leaves, ParquetOptions, PyramidFile, Reducer};
    use arrow_array::cast::AsArray;
    use arrow_array::types::{Float32Type, UInt64Type};
    use arrow_array::Array;

    fn sample() -> Pyramid {
        let cells = grid(13, (10.0, 10.8), (50.0, 50.5), 400);
        let n = cells.len();
        let mut v: Vec<f64> = (0..n).map(|i| (i % 100) as f64 / 7.0).collect();
        v[3] = f64::NAN;
        let leaves = Leaves {
            cells,
            columns: vec![
                Column::new("v", ColumnType::Float32, Reducer::Mean, v.clone()),
                Column::new("r", ColumnType::UInt8, Reducer::RepChild, v),
            ],
            ..Default::default()
        };
        build(leaves, &default_lods(13), &Cancel::new()).unwrap()
    }

    fn opts() -> TileOptions {
        TileOptions {
            tile_size: 1000,
            metadata: vec![("app".into(), "{\"k\":1}".into())],
            ..Default::default()
        }
    }

    fn tempdir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("a5pyramid-tiled-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn batches_hold_every_level_in_tiles() {
        let p = sample();
        let tiled = TiledPyramid::new(p.clone(), &opts(), &Cancel::new()).unwrap();
        let meta = tiled.meta();
        assert_eq!(meta.n_tiles(), tiled.n_tiles());
        assert_eq!(meta.lods(), p.lods());
        let batches: Vec<RecordBatch> = tiled.batches().collect::<Result<_>>().unwrap();
        assert!(batches
            .iter()
            .all(|b| b.num_rows() <= 1000 && b.schema() == tiled.schema()));
        for (level, lm) in p.levels.iter().zip(&meta.levels) {
            let tiles = &batches[lm.tiles[0]..lm.tiles[1]];
            let cells: Vec<u64> = tiles
                .iter()
                .flat_map(|b| b.column(0).as_primitive::<UInt64Type>().values().to_vec())
                .collect();
            assert_eq!(cells, level.cells);
            let v: Vec<f64> = tiles
                .iter()
                .flat_map(|b| {
                    let a = b.column(3).as_primitive::<Float32Type>().clone();
                    (0..a.len())
                        .map(move |i| if a.is_null(i) { f64::NAN } else { a.value(i) as f64 })
                        .collect::<Vec<_>>()
                })
                .collect();
            let same = v
                .iter()
                .zip(&level.values[0])
                .all(|(a, b)| a == b || (a.is_nan() && b.is_nan()));
            assert!(same && v.len() == level.len(), "LOD {}", level.lod);
        }
        let md = tiled.schema().metadata().clone();
        assert_eq!(PyramidMeta::from_json(&md[META_KEY]).unwrap(), *meta);
        assert_eq!(md[A5_INDEX_KEY], A5_INDEX);
        assert_eq!(md["app"], "{\"k\":1}");
        assert_eq!(tiled.metadata()[0].0, META_KEY);
        assert!(tiled.tile(tiled.n_tiles()).is_err());
    }

    #[test]
    fn the_reader_streams_every_tile() {
        let tiled = TiledPyramid::new(sample(), &opts(), &Cancel::new()).unwrap();
        let rows = tiled.tile_rows();
        let schema = tiled.schema();
        let reader = tiled.into_reader();
        assert_eq!(reader.schema(), schema);
        assert_eq!(reader.size_hint(), (rows.len(), Some(rows.len())));
        let got: Vec<usize> = reader.map(|b| b.unwrap().num_rows()).collect();
        assert_eq!(got, rows);
    }

    #[test]
    fn row_group_i_of_the_file_is_tile_i() {
        let dir = tempdir("pq");
        let path = dir.join("p.parquet");
        let tiled = TiledPyramid::new(sample(), &opts(), &Cancel::new()).unwrap();
        let summary = write_parquet(&tiled, &path, &ParquetOptions::default(), &Cancel::new()).unwrap();
        assert_eq!(summary.row_groups, tiled.n_tiles());
        let f = PyramidFile::open(&path).unwrap();
        assert_eq!(f.meta, *tiled.meta());
        assert_eq!(f.metadata, vec![("app".to_string(), "{\"k\":1}".to_string())]);
        for i in 0..tiled.n_tiles() {
            let from_file = f.read(&[i]).unwrap();
            assert_eq!(from_file.len(), 1);
            assert_eq!(from_file[0].columns(), tiled.tile(i).unwrap().columns(), "tile {i}");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn arrow_ipc_keeps_the_tiles_and_index() {
        let tiled = TiledPyramid::new(sample(), &opts(), &Cancel::new()).unwrap();
        let mut buf = Vec::new();
        {
            let mut w = arrow_ipc::writer::FileWriter::try_new(&mut buf, &tiled.schema()).unwrap();
            for b in tiled.batches() {
                w.write(&b.unwrap()).unwrap();
            }
            w.finish().unwrap();
        }
        let r = arrow_ipc::reader::FileReader::try_new(std::io::Cursor::new(buf), None).unwrap();
        assert_eq!(r.num_batches(), tiled.n_tiles());
        let md = r.schema().metadata().clone();
        assert_eq!(PyramidMeta::from_json(&md[META_KEY]).unwrap(), *tiled.meta());
        for (i, b) in r.enumerate() {
            assert_eq!(b.unwrap().columns(), tiled.tile(i).unwrap().columns());
        }
    }

    #[test]
    fn selection_matches_the_file() {
        let tiled = TiledPyramid::new(sample(), &opts(), &Cancel::new()).unwrap();
        let view = Bbox::new(10.1, 50.1, 10.3, 50.2).unwrap();
        let picked = tiled.select(Some(&[13]), Some(&view)).unwrap();
        let all = tiled.select(Some(&[13]), None).unwrap();
        assert!(!picked.is_empty() && picked.len() < all.len());
        assert!(tiled.select(Some(&[1]), None).is_err());
    }

    #[test]
    fn vector_columns_are_fixed_size_binary_in_a_version_3_file() {
        use arrow_array::cast::AsArray;
        let cells = crate::testutil::grid(12, (10.0, 10.5), (50.0, 50.3), 120);
        let n = cells.len();
        let bytes: Vec<u8> = (0..n * 8).map(|i| (i * 31 % 256) as u8).collect();
        let leaves = Leaves {
            cells,
            columns: vec![Column::new("v", ColumnType::Float32, Reducer::Mean, vec![1.0; n])],
            vectors: vec![crate::VectorColumn::new("emb", ColumnType::Int8, 8, bytes.clone())],
            ..Default::default()
        };
        let p = build(leaves, &[8, 10], &Cancel::new()).unwrap();
        let tiled = TiledPyramid::new(p, &opts(), &Cancel::new()).unwrap();
        let m = tiled.meta();
        assert_eq!(m.format_version, 3);
        let emb = m.columns.iter().find(|c| c.name == "emb").unwrap();
        assert_eq!(
            (emb.dim, emb.ty.as_str(), emb.reducer.as_str()),
            (Some(8), "int8", "rep_child")
        );
        assert!(m.columns.iter().find(|c| c.name == "v").unwrap().dim.is_none());
        // the leaves' vectors come back from the file, tile by tile
        let dir = tempdir("vectors");
        let path = dir.join("p.parquet");
        crate::write_parquet(&tiled, &path, &Default::default(), &Cancel::new()).unwrap();
        let f = crate::PyramidFile::open(&path).unwrap();
        assert_eq!(f.meta.format_version, 3);
        let leaf_tiles = f.select(Some(&[12]), None).unwrap();
        let mut back = Vec::new();
        for b in f.read(&leaf_tiles).unwrap() {
            let a = b.column_by_name("emb").unwrap().as_fixed_size_binary();
            assert_eq!(a.value_length(), 8);
            for i in 0..a.len() {
                back.extend_from_slice(a.value(i));
            }
        }
        assert_eq!(back, bytes);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_index_records_codecs_and_mean_vectors() {
        let cells = crate::testutil::grid(12, (10.0, 10.5), (50.0, 50.3), 120);
        let n = cells.len();
        let bytes: Vec<u8> = (0..n * 8).map(|i| (i * 31 % 256) as u8).collect();
        let leaves = Leaves {
            cells,
            columns: vec![
                Column::new("q", ColumnType::Int8, Reducer::Mean, vec![3.0; n]).with_codec(crate::Codec::Aef),
                Column::new("v", ColumnType::Float32, Reducer::Mean, vec![1.0; n]),
            ],
            vectors: vec![crate::VectorColumn::new("emb", ColumnType::Int8, 8, bytes)
                .with_reducer(Reducer::Mean)
                .with_codec(crate::Codec::Aef)],
            ..Default::default()
        };
        let p = build(leaves, &[8, 10], &Cancel::new()).unwrap();
        let tiled = TiledPyramid::new(p, &opts(), &Cancel::new()).unwrap();
        let col = |name: &str| tiled.meta().columns.iter().find(|c| c.name == name).unwrap().clone();
        let (q, v, emb) = (col("q"), col("v"), col("emb"));
        assert_eq!((q.reducer.as_str(), q.codec.as_deref()), ("mean", Some("aef")));
        assert_eq!(v.codec, None);
        assert_eq!(
            (emb.reducer.as_str(), emb.codec.as_deref(), emb.sd, emb.dim),
            ("mean", Some("aef"), None, Some(8))
        );
        let json = tiled.meta().to_json().unwrap();
        assert_eq!(json.matches("\"codec\":\"aef\"").count(), 2);
        let dir = tempdir("codecs");
        let path = dir.join("p.parquet");
        crate::write_parquet(&tiled, &path, &Default::default(), &Cancel::new()).unwrap();
        let f = crate::PyramidFile::open(&path).unwrap();
        assert_eq!(f.meta.columns, tiled.meta().columns);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn files_without_vectors_keep_version_1() {
        let tiled = TiledPyramid::new(sample(), &opts(), &Cancel::new()).unwrap();
        assert_eq!(tiled.meta().format_version, 1);
    }

    #[test]
    fn bad_options_are_refused() {
        let cancel = Cancel::new();
        let zero = TileOptions {
            tile_size: 0,
            ..Default::default()
        };
        assert!(TiledPyramid::new(sample(), &zero, &cancel).is_err());
        for key in RESERVED_KEYS {
            let o = TileOptions {
                metadata: vec![(key.into(), "x".into())],
                ..Default::default()
            };
            assert!(TiledPyramid::new(sample(), &o, &cancel).is_err());
        }
        let dup = TileOptions {
            metadata: vec![("a".into(), "1".into()), ("a".into(), "2".into())],
            ..Default::default()
        };
        assert!(TiledPyramid::new(sample(), &dup, &cancel).is_err());
        let stop = Cancel::new();
        stop.cancel();
        assert!(matches!(
            TiledPyramid::new(sample(), &TileOptions::default(), &stop),
            Err(Error::Cancelled)
        ));
    }
}
