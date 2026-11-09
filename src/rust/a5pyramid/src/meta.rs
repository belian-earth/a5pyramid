//! The index a pyramid carries: in a Parquet file's key-value metadata, or
//! an Arrow schema's metadata.

use serde::{Deserialize, Serialize};

use crate::bbox::Bbox;
use crate::error::{invalid, Error, Result};

/// Metadata key of the JSON index ([`PyramidMeta`]).
pub const META_KEY: &str = "a5pyramid";
/// Metadata key recording the A5 index version of the cell ids, shared
/// across the a5R / a5px / a5view stack.
pub const A5_INDEX_KEY: &str = "a5_index";
/// The A5 index version of the ids this crate writes. A5 1.0 renumbered
/// every cell, and an id does not say which version wrote it.
pub const A5_INDEX: &str = "1.0";
/// The version of the layout this crate writes, and the newest it reads.
pub const FORMAT_VERSION: u32 = 1;

/// The index: what the levels are, which tiles hold each, and what area
/// each tile covers. A tile is one row group of a Parquet file and one
/// record batch in Arrow, numbered in order from 0.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PyramidMeta {
    pub format_version: u32,
    /// The software that wrote the pyramid, e.g. `"a5pyramid 0.0.0"`.
    pub generator: String,
    /// The a5 crate version that computed the supercells and boundaries.
    pub a5_crate: String,
    /// The resolution of the leaves, the finest level.
    pub data_resolution: i32,
    /// The most rows a tile holds.
    pub tile_size: usize,
    /// `[west, south, east, north]` of all the leaves (`west > east` across
    /// the antimeridian).
    pub bbox: [f64; 4],
    pub columns: Vec<ColumnMeta>,
    /// One entry per level, in ascending LOD order (the leaves last).
    pub levels: Vec<LevelMeta>,
    /// `[west, south, east, north]` of each tile in order, flat: tile `i` is
    /// at `4 * i .. 4 * i + 4`. Named `row_group_bbox` by writers before
    /// 2026-11-09, which readers accept.
    #[serde(alias = "row_group_bbox")]
    pub tile_bbox: Vec<f64>,
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
/// `[start, end)` of its tiles. Levels follow each other.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LevelMeta {
    pub lod: i32,
    pub rows: u64,
    /// Named `row_groups` by writers before 2026-11-09, which readers accept.
    #[serde(alias = "row_groups")]
    pub tiles: [usize; 2],
}

impl PyramidMeta {
    pub fn from_json(s: &str) -> Result<Self> {
        let meta: PyramidMeta = serde_json::from_str(s)?;
        if meta.format_version > FORMAT_VERSION {
            return Err(Error::Format(format!(
                "the pyramid is in a5pyramid format version {}, newer than version {FORMAT_VERSION} this reader understands",
                meta.format_version
            )));
        }
        Ok(meta)
    }

    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string(self)?)
    }

    pub fn n_tiles(&self) -> usize {
        self.tile_bbox.len() / 4
    }

    pub fn lods(&self) -> Vec<i32> {
        self.levels.iter().map(|l| l.lod).collect()
    }

    /// The bounding box of tile `i`.
    pub fn tile_bbox(&self, i: usize) -> Bbox {
        let b = &self.tile_bbox[4 * i..4 * i + 4];
        Bbox {
            west: b[0],
            south: b[1],
            east: b[2],
            north: b[3],
        }
    }

    /// The level holding tile `i`.
    pub fn level_of(&self, i: usize) -> Option<&LevelMeta> {
        self.levels.iter().find(|l| i >= l.tiles[0] && i < l.tiles[1])
    }

    /// The tiles at `lods` (all levels when `None`) whose boxes meet `bbox`
    /// (everywhere when `None`), in order: what a renderer fetches for a
    /// view. Every LOD must be one of the pyramid's.
    pub fn select(&self, lods: Option<&[i32]>, bbox: Option<&Bbox>) -> Result<Vec<usize>> {
        let mut levels: Vec<&LevelMeta> = match lods {
            None => self.levels.iter().collect(),
            Some(lods) => {
                lods.iter()
                    .map(|&lod| {
                        self.levels.iter().find(|l| l.lod == lod).ok_or_else(|| {
                            invalid(format!("the pyramid has no LOD {lod}; its LODs are {:?}", self.lods()))
                        })
                    })
                    .collect::<Result<_>>()?
            }
        };
        levels.sort_by_key(|l| l.tiles[0]);
        levels.dedup_by_key(|l| l.lod);
        Ok(levels
            .into_iter()
            .flat_map(|l| l.tiles[0]..l.tiles[1])
            .filter(|&i| bbox.is_none_or(|b| self.tile_bbox(i).intersects(b)))
            .collect())
    }

    /// Check that the index agrees with a pyramid of `n_tiles` tiles (row
    /// groups of a file, record batches in Arrow).
    pub fn validate(&self, n_tiles: usize) -> Result<()> {
        let bad = |msg: String| Err(Error::Format(format!("inconsistent a5pyramid index: {msg}")));
        if self.tile_bbox.len() != 4 * n_tiles {
            return bad(format!("{} tile boxes for {n_tiles} tiles", self.tile_bbox.len() / 4));
        }
        let mut next = 0;
        for l in &self.levels {
            if l.tiles[0] != next || l.tiles[1] < l.tiles[0] {
                return bad(format!("level {} does not follow the one before", l.lod));
            }
            next = l.tiles[1];
        }
        if next != n_tiles {
            return bad(format!("levels hold {next} of {n_tiles} tiles"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexes_written_before_the_rename_still_read() {
        let old = r#"{"format_version":1,"generator":"g","a5_crate":"1.0.0","data_resolution":5,
            "tile_size":10,"bbox":[0,0,1,1],"columns":[],
            "levels":[{"lod":5,"rows":3,"row_groups":[0,1]}],"row_group_bbox":[0,0,1,1]}"#;
        let meta = PyramidMeta::from_json(old).unwrap();
        assert_eq!(meta.levels[0].tiles, [0, 1]);
        assert_eq!(meta.n_tiles(), 1);
        let json = meta.to_json().unwrap();
        assert!(json.contains("\"tiles\":[0,1]") && json.contains("\"tile_bbox\""));
        assert!(!json.contains("row_group"));
    }
}
