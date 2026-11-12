//! Level-of-detail pyramids of [A5](https://a5geo.org/) cells.
//!
//! A pyramid holds a set of A5 cells at one resolution (the *leaves*, with
//! their values) together with coarser *levels of detail* (LODs) built from
//! them, in one Parquet file laid out for a browser to read with range
//! requests: it fetches the footer, picks the LOD for the current zoom and
//! the row groups whose bounding boxes meet the viewport, and decodes only
//! those. The same tiles are available in memory as Arrow record batches.
//! The layout is specified in `FORMAT.md`.
//!
//! Building has three steps:
//!
//! 1. [`build()`] groups the leaves under each coarser LOD by
//!    [`a5::cell_to_supercell`], the coarse cell containing a leaf's centre,
//!    and reduces each column within each group ([`Reducer`]). Every LOD is
//!    grouped from the leaves directly: supercells are not transitive (a
//!    leaf's supercell two levels up is not always the supercell of its
//!    supercell), so a LOD aggregated from the one below would misplace
//!    values.
//! 2. [`TiledPyramid::new()`] splits each level into tiles of contiguous
//!    cell ids ([`plan_tiles()`]), computes each tile's bounding box from the
//!    cell boundaries, and indexes them. Each tile is one Arrow record batch,
//!    with the index in the schema metadata: the pyramid in memory, as a
//!    renderer reads it. [`TiledPyramid::into_reader()`] streams the batches
//!    as an [`arrow_array::RecordBatchReader`] for other Arrow libraries.
//! 3. Optionally, [`write_parquet()`] writes the tiles as row groups of one
//!    Parquet file, with the index in its key-value metadata.
//!
//! [`PyramidFile`] reads a file's index back and selects and decodes row
//! groups, as a renderer would; [`PyramidMeta::select`] does the same
//! selection on an in-memory index.
//!
//! Work runs on the current rayon thread pool; run inside
//! `ThreadPool::install` to choose the thread count. Results do not depend
//! on the number of threads.

mod bbox;
mod build;
mod cancel;
mod column;
mod error;
mod meta;
mod partials;
mod read;
mod set;
mod set_table;
mod tiled;
mod tiles;
#[cfg(test)]
mod v4_tests;
mod write;

pub use bbox::Bbox;
pub use build::{build, default_lods, Leaves, Level, Pyramid};
pub use cancel::Cancel;
pub use column::{
    Codec, Column, ColumnSpec, ColumnType, Reducer, VectorColumn, VectorSpec, VectorStorage, WeightsSpec,
};
pub use error::{Error, Result};
pub use meta::{
    ColumnMeta, LevelMeta, PyramidMeta, ShardMeta, TileLocation, WeightsMeta, A5_INDEX, A5_INDEX_KEY, FORMAT_VERSION,
    META_KEY,
};
pub use partials::{
    merge_partials, partials, partials_meta, PartialsMerge, PartialsMeta, PartialsVector, PartialsWeights, KEY_COLUMN,
    PARTIALS_META_KEY, PARTIALS_VERSION,
};
pub use read::PyramidFile;
pub use set::{
    default_tiers, write_set, SetIndexFormat, SetMeta, SetOptions, ShardEntry, Tier, DEFAULT_SET_TILE_SIZE, SET_FORMAT,
    SET_INDEX,
};
pub use set_table::{read_set_table, write_set_table, SET_META_KEY, SET_TABLE, SET_TABLE_COLUMNS};
pub use tiled::{TileOptions, TileReader, TiledPyramid};
pub use tiles::plan_tiles;
pub use write::{write_parquet, Compression, ParquetOptions, WriteSummary};

/// The version of the a5 crate this crate is built with: the exact pin in
/// `Cargo.toml`, which a unit test holds this to. Recorded in every file.
pub const A5_CRATE_VERSION: &str = "1.0.0";

/// Name of the cell id column (`uint64`).
pub const CELL_COLUMN: &str = "cell";
/// Name of the column holding each row's LOD, the A5 resolution of its cell.
pub const LOD_COLUMN: &str = "lod";
/// Name of the column holding the number of leaves each row summarises.
pub const NCELL_COLUMN: &str = "ncell";
/// Name of the column holding, in a shard with partial rows, the cell at
/// the shard resolution containing each row cell's centre (format version 4).
pub const OWNER_COLUMN: &str = "owner";

/// Column names a value column may not take.
pub const RESERVED_COLUMNS: [&str; 4] = [CELL_COLUMN, LOD_COLUMN, NCELL_COLUMN, OWNER_COLUMN];

/// A cell id as 16 zero-padded hex digits, as a5R and a5-js print them.
pub(crate) fn hex(cell: u64) -> String {
    format!("{cell:016x}")
}

#[cfg(test)]
pub(crate) mod testutil {
    use std::collections::HashSet;

    /// Every resolution-`res` cell containing a point of a `steps` x `steps`
    /// lattice over a lon/lat box, in ascending id order.
    pub(crate) fn grid(res: i32, lon: (f64, f64), lat: (f64, f64), steps: usize) -> Vec<u64> {
        let mut cells = HashSet::new();
        for a in 0..steps {
            for b in 0..steps {
                let x = lon.0 + (lon.1 - lon.0) * a as f64 / (steps - 1) as f64;
                let y = lat.0 + (lat.1 - lat.0) * b as f64 / (steps - 1) as f64;
                cells.insert(a5::lonlat_to_cell(a5::LonLat::new(x, y), res).unwrap());
            }
        }
        let mut v: Vec<u64> = cells.into_iter().collect();
        v.sort_unstable();
        v
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn crate_version_matches_the_cargo_pin() {
        let toml = include_str!("../Cargo.toml");
        let pin = format!("a5 = '={}'", super::A5_CRATE_VERSION);
        assert!(toml.contains(&pin), "Cargo.toml does not pin {pin}");
    }
}
