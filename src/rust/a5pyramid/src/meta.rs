//! The index a pyramid file carries in its Parquet key-value metadata.

use serde::{Deserialize, Serialize};

use crate::bbox::Bbox;
use crate::error::{Error, Result};

/// Key-value metadata key of the JSON index ([`PyramidMeta`]).
pub const META_KEY: &str = "a5pyramid";
/// Key-value metadata key recording the A5 index version of the cell ids,
/// shared across the a5R / a5px / a5view stack.
pub const A5_INDEX_KEY: &str = "a5_index";
/// The A5 index version of the ids this crate writes. A5 1.0 renumbered
/// every cell, and an id does not say which version wrote it.
pub const A5_INDEX: &str = "1.0";
/// The version of the file layout this crate writes, and the newest it
/// reads.
pub const FORMAT_VERSION: u32 = 1;

/// The file's index: what the levels are, where their row groups are, and
/// what area each row group covers.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PyramidMeta {
    pub format_version: u32,
    /// The software that wrote the file, e.g. `"a5pyramid 0.0.0"`.
    pub generator: String,
    /// The a5 crate version that computed the supercells and boundaries.
    pub a5_crate: String,
    /// The resolution of the leaves, the finest level.
    pub data_resolution: i32,
    /// The most rows a row group holds.
    pub tile_size: usize,
    /// `[west, south, east, north]` of all the leaves (`west > east` across
    /// the antimeridian).
    pub bbox: [f64; 4],
    pub columns: Vec<ColumnMeta>,
    /// One entry per level, in ascending LOD order (the leaves last).
    pub levels: Vec<LevelMeta>,
    /// `[west, south, east, north]` of each row group in file order, flat:
    /// row group `i` is at `4 * i .. 4 * i + 4`.
    pub row_group_bbox: Vec<f64>,
}

/// A value column: its name, storage type and reducer.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ColumnMeta {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub reducer: String,
}

/// A level: its LOD (A5 resolution), row count, and the half-open range
/// `[start, end)` of its row groups. Levels follow each other in the file.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LevelMeta {
    pub lod: i32,
    pub rows: u64,
    pub row_groups: [usize; 2],
}

impl PyramidMeta {
    pub fn from_json(s: &str) -> Result<Self> {
        let meta: PyramidMeta = serde_json::from_str(s)?;
        if meta.format_version > FORMAT_VERSION {
            return Err(Error::Format(format!(
                "the file is in a5pyramid format version {}, newer than version {FORMAT_VERSION} this reader understands",
                meta.format_version
            )));
        }
        Ok(meta)
    }

    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string(self)?)
    }

    pub fn n_row_groups(&self) -> usize {
        self.row_group_bbox.len() / 4
    }

    pub fn lods(&self) -> Vec<i32> {
        self.levels.iter().map(|l| l.lod).collect()
    }

    pub fn row_group_bbox(&self, rg: usize) -> Bbox {
        let b = &self.row_group_bbox[4 * rg..4 * rg + 4];
        Bbox {
            west: b[0],
            south: b[1],
            east: b[2],
            north: b[3],
        }
    }

    /// The level holding row group `rg`.
    pub fn level_of(&self, rg: usize) -> Option<&LevelMeta> {
        self.levels
            .iter()
            .find(|l| rg >= l.row_groups[0] && rg < l.row_groups[1])
    }

    /// Check that the index agrees with a file of `n_row_groups` row groups.
    pub fn validate(&self, n_row_groups: usize) -> Result<()> {
        let bad = |msg: String| Err(Error::Format(format!("inconsistent a5pyramid index: {msg}")));
        if self.row_group_bbox.len() != 4 * n_row_groups {
            return bad(format!(
                "{} row group boxes for {n_row_groups} row groups",
                self.row_group_bbox.len() / 4
            ));
        }
        let mut next = 0;
        for l in &self.levels {
            if l.row_groups[0] != next || l.row_groups[1] < l.row_groups[0] {
                return bad(format!("level {} does not follow the one before", l.lod));
            }
            next = l.row_groups[1];
        }
        if next != n_row_groups {
            return bad(format!("levels hold {next} of {n_row_groups} row groups"));
        }
        Ok(())
    }
}
