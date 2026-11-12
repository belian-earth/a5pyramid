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
/// The newest version of the layout this crate writes and reads. A writer
/// records the oldest version whose rules a file follows: a single pyramid
/// file is version 1, a shard of a pyramid set version 2, either with a
/// vector column version 3, and either with row groups of several tiles
/// version 4.
pub const FORMAT_VERSION: u32 = 4;
/// The version of a single pyramid file, whose finest level is the leaves.
pub(crate) const SINGLE_FILE_VERSION: u32 = 1;
/// The version of a shard of a set and of a set's index: fixed, so a later
/// version of the format does not make readers of this one refuse them.
pub(crate) const SHARD_FILE_VERSION: u32 = 2;
/// The version of a file or set with a vector column, fixed as the others.
pub(crate) const VECTOR_FILE_VERSION: u32 = 3;
/// The version of a file with a row group of several tiles, and of a set
/// with such a shard, fixed as the others.
pub(crate) const ROW_GROUP_FILE_VERSION: u32 = 4;

/// The index: what the levels are, which tiles hold each, and what area
/// each tile covers. A tile is one record batch in Arrow and, in a Parquet
/// file, one row group or (format version 4) one data page of a row group
/// of several tiles; tiles are numbered in order from 0.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PyramidMeta {
    pub format_version: u32,
    /// The software that wrote the pyramid, e.g. `"a5pyramid 0.0.0"`.
    pub generator: String,
    /// The a5 crate version that computed the supercells and boundaries.
    pub a5_crate: String,
    /// The resolution of the leaves. In a single file it is the finest
    /// level; a shard of a set may hold only coarser levels.
    pub data_resolution: i32,
    /// The most rows a tile holds.
    pub tile_size: usize,
    /// In a file whose row groups hold several tiles (format version 4),
    /// the most rows a row group holds. Row group `g` of `n_g` rows then
    /// holds `ceil(n_g / tile_size)` consecutive tiles, each `tile_size`
    /// rows but the last, and each one data page of every column chunk.
    /// Absent where every row group is one tile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row_group_size: Option<usize>,
    /// `[west, south, east, north]` (`west > east` across the antimeridian):
    /// in a single file the box of the leaves, in a shard the box of every
    /// tile, coarse cells included.
    pub bbox: [f64; 4],
    pub columns: Vec<ColumnMeta>,
    /// One entry per level, in ascending LOD order (the leaves last).
    pub levels: Vec<LevelMeta>,
    /// `[west, south, east, north]` of each tile in order, flat: tile `i` is
    /// at `4 * i .. 4 * i + 4`. Named `row_group_bbox` by writers before
    /// 2026-11-09, which readers accept.
    #[serde(alias = "row_group_bbox")]
    pub tile_bbox: Vec<f64>,
    /// In a shard of a pyramid set (format version 2), which shard it is;
    /// absent in a single file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shard: Option<ShardMeta>,
}

/// Which shard of a pyramid set a file is.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ShardMeta {
    /// The tier, numbered from 0 in ascending LOD order.
    pub tier: usize,
    /// The shard cell, as 16 hex digits: every cell in the file has its
    /// centre in it.
    pub cell: String,
    /// The shard cell's resolution, the tier's shard resolution.
    pub resolution: i32,
}

/// The index entries of value columns: the scalar columns, then the vector
/// columns.
pub(crate) fn column_meta(columns: &[crate::ColumnSpec], vectors: &[crate::VectorSpec]) -> Vec<ColumnMeta> {
    let scalars = columns.iter().map(|c| ColumnMeta {
        name: c.name.clone(),
        ty: c.ty.as_str().to_string(),
        reducer: c.reducer.as_str().to_string(),
        codec: c.codec.map(|k| k.as_str().to_string()),
        sd: c.sd,
        dim: None,
    });
    let vecs = vectors.iter().map(|v| ColumnMeta {
        name: v.name.clone(),
        ty: v.ty.as_str().to_string(),
        reducer: v.reducer.as_str().to_string(),
        codec: v.codec.map(|k| k.as_str().to_string()),
        sd: v.sd,
        dim: Some(v.dim),
    });
    scalars.chain(vecs).collect()
}

/// The format version a file or set records over `base`, its version
/// without vector columns and row groups of several tiles.
pub(crate) fn version_with(vectors: &[crate::VectorSpec], row_groups_of_tiles: bool, base: u32) -> u32 {
    if row_groups_of_tiles {
        ROW_GROUP_FILE_VERSION
    } else if !vectors.is_empty() {
        VECTOR_FILE_VERSION
    } else {
        base
    }
}

/// A value column: its name, storage type and reducer.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ColumnMeta {
    pub name: String,
    #[serde(rename = "type")]
    pub ty: String,
    pub reducer: String,
    /// The codec of a `mean` column whose values are stored as codes, such
    /// as `"aef"` ([`crate::Codec`]): coarse cells hold the code of the
    /// mean of their leaves' decoded values. Absent for other columns and
    /// in files written before 2026-11-11.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codec: Option<String>,
    /// For a `rep_child` column, the standard deviation that scaled it in
    /// the choice of representatives ([`crate::ColumnSpec::sd`]); absent for
    /// other reducers and in files written before 2026-11-09.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sd: Option<f64>,
    /// For a vector column (format version 3), its elements per cell: the
    /// column is fixed-length binary of `dim` bytes, each an element of
    /// `type` (`int8` or `uint8`). Absent for scalar columns.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dim: Option<usize>,
}

/// Where a tile is in a Parquet file: its row group, its first row there,
/// and its rows. In a row group of several tiles (format version 4) the
/// tile is data page `page` of every column chunk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileLocation {
    pub row_group: usize,
    pub page: usize,
    pub offset: u64,
    pub rows: u64,
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

    /// The tiles of a file whose row groups hold `row_group_rows` rows, in
    /// order, after checking that the index agrees with them: each row group
    /// is one tile, or (with `row_group_size`) `ceil(rows / tile_size)` of
    /// them, and no row group holds tiles of two levels.
    pub fn locate_tiles(&self, row_group_rows: &[u64]) -> Result<Vec<TileLocation>> {
        let bad = |msg: String| Err(Error::Format(format!("inconsistent a5pyramid index: {msg}")));
        let mut tiles = Vec::new();
        let mut starts = Vec::with_capacity(row_group_rows.len() + 1);
        for (g, &rows) in row_group_rows.iter().enumerate() {
            starts.push(tiles.len());
            let n = match self.row_group_size {
                None => 1,
                Some(size) => {
                    if rows == 0 || rows > size as u64 {
                        return bad(format!(
                            "row group {g} holds {rows} rows, outside 1 to the row group size {size}"
                        ));
                    }
                    rows.div_ceil(self.tile_size as u64) as usize
                }
            };
            let step = if n == 1 { rows } else { self.tile_size as u64 };
            for page in 0..n {
                let offset = page as u64 * step;
                tiles.push(TileLocation {
                    row_group: g,
                    page,
                    offset,
                    rows: step.min(rows - offset),
                });
            }
        }
        starts.push(tiles.len());
        self.validate(tiles.len())?;
        for l in &self.levels {
            if l.tiles[0] < l.tiles[1]
                && (starts.binary_search(&l.tiles[0]).is_err() || starts.binary_search(&l.tiles[1]).is_err())
            {
                return bad(format!("a row group holds tiles of level {} and another", l.lod));
            }
        }
        Ok(tiles)
    }

    /// Check that the index agrees with a pyramid of `n_tiles` tiles (record
    /// batches in Arrow; see [`Self::locate_tiles`] for a file).
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

    fn index(tile_size: usize, row_group_size: Option<usize>, levels: &[(i32, [usize; 2])]) -> PyramidMeta {
        let n = levels.last().map_or(0, |l| l.1[1]);
        PyramidMeta {
            format_version: 4,
            generator: "g".into(),
            a5_crate: "1.0.0".into(),
            data_resolution: 9,
            tile_size,
            row_group_size,
            bbox: [0.0, 0.0, 1.0, 1.0],
            columns: vec![],
            levels: levels
                .iter()
                .map(|&(lod, tiles)| LevelMeta { lod, rows: 0, tiles })
                .collect(),
            tile_bbox: vec![0.0; 4 * n],
            shard: None,
        }
    }

    #[test]
    fn row_groups_hold_tiles_of_tile_size_rows() {
        let m = index(10, Some(30), &[(5, [0, 1]), (9, [1, 5])]);
        let t = m.locate_tiles(&[4, 30, 7]).unwrap();
        let got: Vec<(usize, usize, u64, u64)> = t.iter().map(|t| (t.row_group, t.page, t.offset, t.rows)).collect();
        assert_eq!(
            got,
            vec![
                (0, 0, 0, 4),
                (1, 0, 0, 10),
                (1, 1, 10, 10),
                (1, 2, 20, 10),
                (2, 0, 0, 7)
            ]
        );
        // too many rows, too few tiles, a row group across two levels
        assert!(m.locate_tiles(&[4, 31, 7]).is_err());
        assert!(m.locate_tiles(&[4, 30]).is_err());
        assert!(index(10, Some(30), &[(5, [0, 2]), (9, [2, 5])])
            .locate_tiles(&[4, 30, 7])
            .is_err());
        // without a row group size every row group is one tile
        let one = index(10, None, &[(5, [0, 1]), (9, [1, 3])]);
        assert_eq!(one.locate_tiles(&[4, 10, 7]).unwrap().len(), 3);
        assert!(one.locate_tiles(&[4, 10]).is_err());
    }

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
