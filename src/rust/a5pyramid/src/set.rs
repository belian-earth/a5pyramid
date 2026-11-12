//! Pyramid sets: one pyramid stored as many files.
//!
//! A single pyramid file holds every tile of every level, and a browser
//! reads its whole footer, which grows by a few hundred bytes a tile,
//! before it can draw anything. A set splits the pyramid so that every
//! file stays small and a view needs only a few of them.
//!
//! The LODs are split into *tiers*, contiguous ranges of LODs. Each tier
//! has a *shard resolution* `s`, no finer than its coarsest LOD: a cell `C`
//! of the tier belongs to the *shard* `a5::cell_to_supercell(C, s)`, the
//! cell at `s` containing `C`'s centre. Every cell is therefore in exactly
//! one shard. Each shard is one pyramid file (format version 2) holding
//! its tier's LODs for the cells it owns, and the set's index, `set.json`,
//! lists the tiers and every shard with its path, size and box.
//!
//! Cells are grouped into levels before the pyramid is split, so every
//! coarse cell summarises all of its leaves, whichever shard they are in.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::bbox::Bbox;
use crate::build::{Level, Pyramid};
use crate::cancel::Cancel;
use crate::error::{invalid, Error, Result};
use crate::meta::{
    column_meta, version_with, ColumnMeta, ShardMeta, WeightsMeta, A5_INDEX, FORMAT_VERSION, SHARD_FILE_VERSION,
    VERSION_4,
};
use crate::read::PyramidFile;
use crate::set_table::{read_set_table, write_set_table, SET_TABLE};
use crate::tiled::{TileOptions, TiledPyramid};
use crate::write::{write_parquet, ParquetOptions};

/// File name of a set's index, in the set's directory.
pub const SET_INDEX: &str = "set.json";
/// The `format` a set's index declares.
pub const SET_FORMAT: &str = "a5pyramid-set";
/// Tile size of the tiers [`default_tiers`] makes.
pub const DEFAULT_SET_TILE_SIZE: usize = 16384;

/// One tier of a set: the LODs it holds (`lods[0]` to `lods[1]`,
/// inclusive), the resolution of the cells that key its shards, the most
/// rows in each of its tiles, and optionally the most rows in a row group
/// of several tiles ([`TileOptions::row_group_size`]).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Tier {
    pub lods: [i32; 2],
    pub shard_resolution: i32,
    pub tile_size: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub row_group_size: Option<usize>,
}

impl Tier {
    fn holds(&self, lod: i32) -> bool {
        lod >= self.lods[0] && lod <= self.lods[1]
    }
}

/// The tiers of a pyramid with leaves at `data_resolution` and coarser
/// levels at `lods`: from the leaves down, three LODs a tier, each keyed by
/// cells 12 resolutions coarser than its finest LOD, and every LOD left
/// over in one tier keyed at resolution 0.
///
/// With a shard resolution 12 below the finest LOD, a shard covered by data
/// holds about 4^12 (16.8 M) cells at that LOD and a third as many again at
/// the tier's coarser two. A map view spans about 1,300 cells across at the
/// LOD drawn, so it meets at most a few shards of the tier at any zoom. For
/// leaves at resolution 18 the tiers are 16 to 18 (shards at resolution 6),
/// 13 to 15 (at 3) and the rest (at 0). Tiers holding none of the levels
/// are left out.
pub fn default_tiers(data_resolution: i32, lods: &[i32]) -> Vec<Tier> {
    let low = lods
        .iter()
        .copied()
        .min()
        .unwrap_or(data_resolution)
        .min(data_resolution);
    let mut tiers = Vec::new();
    let mut hi = data_resolution;
    while hi - 12 > 0 && hi - 2 > low {
        tiers.push(Tier {
            lods: [hi - 2, hi],
            shard_resolution: hi - 12,
            tile_size: DEFAULT_SET_TILE_SIZE,
            row_group_size: None,
        });
        hi -= 3;
    }
    if hi >= low {
        tiers.push(Tier {
            lods: [low, hi],
            shard_resolution: (hi - 12).max(0).min(low),
            tile_size: DEFAULT_SET_TILE_SIZE,
            row_group_size: None,
        });
    }
    tiers.reverse();
    let mut all: Vec<i32> = lods.to_vec();
    all.push(data_resolution);
    tiers.retain(|t| all.iter().any(|&l| t.holds(l)));
    tiers
}

#[derive(Clone, Debug)]
pub struct SetOptions {
    /// The tiers, which must hold every level of the pyramid between them,
    /// each at least one; see [`default_tiers`].
    pub tiers: Vec<Tier>,
    /// Recorded as each shard's and the index's `generator`.
    pub generator: String,
    /// Extra metadata, written into every shard file's key-value metadata
    /// and the index's `metadata`. The same keys are reserved as for a
    /// single file.
    pub metadata: Vec<(String, String)>,
    pub parquet: ParquetOptions,
    /// How to write the set's index.
    pub index: SetIndexFormat,
}

/// The form of a set's index.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SetIndexFormat {
    /// `set.json`, one document (the default).
    #[default]
    Json,
    /// `set.parquet`, a shard table (format version 4; see
    /// [`crate::write_set_table`]).
    Parquet,
}

/// The index of a set: `set.json` in its directory, or the footer and rows
/// of its `set.parquet`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SetMeta {
    /// Always [`SET_FORMAT`].
    pub format: String,
    pub format_version: u32,
    pub generator: String,
    pub a5_crate: String,
    /// The A5 index version of the cell ids, as in each shard's
    /// `a5_index` metadata.
    pub a5_index: String,
    pub data_resolution: i32,
    pub columns: Vec<ColumnMeta>,
    /// Version 4: the weights column every shard stores.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weights: Option<WeightsMeta>,
    /// In ascending LOD order; a tier's number is its position.
    pub tiers: Vec<Tier>,
    /// The box of every shard's box.
    pub bbox: [f64; 4],
    /// Every shard, by tier and then shard cell.
    pub shards: Vec<ShardEntry>,
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

/// A shard as the index lists it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ShardEntry {
    pub tier: usize,
    /// The shard cell, 16 hex digits.
    pub cell: String,
    /// The file, relative to the set's directory, with `/` separators;
    /// `None` for a shard listed as empty, with no file (version 4).
    #[serde(default)]
    pub path: Option<String>,
    pub rows: u64,
    pub tiles: usize,
    /// The file's size.
    pub bytes: u64,
    /// Bytes from the end of the last column chunk to the end of the file:
    /// the Parquet metadata (the shard's index included), its 4-byte length
    /// and the closing `PAR1`. A reader that fetches the last `footer_bytes`
    /// bytes has the whole footer in one request.
    pub footer_bytes: u64,
    /// The box of every tile in the shard; `None` for an empty shard.
    #[serde(default)]
    pub bbox: Option<[f64; 4]>,
}

impl SetMeta {
    /// Parse an index, refusing another format or a newer version.
    pub fn from_json(s: &str) -> Result<Self> {
        let meta: SetMeta = serde_json::from_str(s)?;
        if meta.format != SET_FORMAT {
            return Err(Error::Format(format!(
                "not an a5pyramid set index: format is {:?}, not {SET_FORMAT:?}",
                meta.format
            )));
        }
        if meta.format_version > FORMAT_VERSION {
            return Err(Error::Format(format!(
                "the set is in a5pyramid format version {}, newer than version {FORMAT_VERSION} this reader understands",
                meta.format_version
            )));
        }
        Ok(meta)
    }

    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string(self)?)
    }

    /// Read the index of the set at `path`: its directory (holding
    /// `set.json` or `set.parquet`), or one of those files.
    pub fn read(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let file = if path.is_dir() {
            if path.join(SET_INDEX).exists() || !path.join(SET_TABLE).exists() {
                path.join(SET_INDEX)
            } else {
                path.join(SET_TABLE)
            }
        } else {
            path.to_path_buf()
        };
        if file.extension().is_some_and(|e| e == "parquet") {
            return Ok(read_set_table(&file)?.0);
        }
        Self::from_json(&std::fs::read_to_string(&file)?)
    }

    /// The index of a set whose shards were written one at a time, such as
    /// a store's files: its own fields from one shard (`template`), its
    /// columns, weights, leaves' resolution and a5 crate, which every shard
    /// shares; `tiers` in ascending LOD order; `shards` by tier and cell,
    /// a shard without a file listed empty. It is version 4, for a
    /// [`crate::write_set_table`].
    pub fn assemble(
        template: &PyramidFile,
        tiers: Vec<Tier>,
        shards: Vec<ShardEntry>,
        generator: String,
        metadata: BTreeMap<String, String>,
    ) -> Result<Self> {
        for w in tiers.windows(2) {
            if w[1].lods[0] <= w[0].lods[1] {
                return Err(invalid("tiers must be in ascending LOD order and not overlap"));
            }
        }
        if let Some(t) = tiers.iter().find(|t| t.tile_size == 0 || t.lods[0] > t.lods[1]) {
            return Err(invalid(format!(
                "the tier of LODs {} to {} is not sound",
                t.lods[0], t.lods[1]
            )));
        }
        let m = &template.meta;
        let boxes: Vec<Bbox> = shards
            .iter()
            .filter_map(|s| s.bbox)
            .map(Bbox::from_array)
            .collect::<Result<_>>()?;
        let meta = SetMeta {
            format: SET_FORMAT.to_string(),
            format_version: VERSION_4,
            generator,
            a5_crate: m.a5_crate.clone(),
            a5_index: template.a5_index.clone().unwrap_or_else(|| A5_INDEX.to_string()),
            data_resolution: m.data_resolution,
            columns: m.columns.clone(),
            weights: m.weights.clone(),
            tiers,
            bbox: Bbox::covering(&boxes).unwrap_or(Bbox::WORLD).to_array(),
            shards,
            metadata,
        };
        crate::set_table::check_shards(&meta)?;
        Ok(meta)
    }

    /// The tier holding `lod`, if any.
    pub fn tier_of(&self, lod: i32) -> Option<usize> {
        self.tiers.iter().position(|t| t.holds(lod))
    }

    /// The shards of `tier` whose boxes meet `bbox` (every shard of the
    /// tier when `None`), as positions in [`SetMeta::shards`]: the files a
    /// reader opens for a view drawn at one of the tier's LODs.
    pub fn select(&self, tier: usize, bbox: Option<&Bbox>) -> Vec<usize> {
        self.shards
            .iter()
            .enumerate()
            .filter(|(_, s)| s.tier == tier && s.path.is_some())
            .filter(|(_, s)| {
                let Some([west, south, east, north]) = s.bbox else {
                    return false;
                };
                bbox.is_none_or(|b| {
                    Bbox {
                        west,
                        south,
                        east,
                        north,
                    }
                    .intersects(b)
                })
            })
            .map(|(i, _)| i)
            .collect()
    }
}

/// Write `pyramid` as a set in the directory `dir`, which must not exist or
/// be empty: one shard file per tier and shard cell holding cells, at
/// `tier=<t>/shard=<cell>.parquet`, then `set.json`. The index is written
/// last and renamed into place, so a reader never finds an index that
/// names a missing shard. A write that fails or is cancelled removes what it
/// wrote (and `dir`, if it created it), so it can be run again.
///
/// Shards are written one at a time, each tiled in parallel, and each
/// tier's levels are released once its shards are written.
pub fn write_set(pyramid: Pyramid, dir: impl AsRef<Path>, opts: &SetOptions, cancel: &Cancel) -> Result<SetMeta> {
    let dir = dir.as_ref();
    let tiers = check_tiers(&opts.tiers, &pyramid.lods())?;
    let created = prepare_dir(dir)?;
    let mut written = Written::default();
    match write_shards(pyramid, dir, tiers, opts, cancel, &mut written) {
        Ok(meta) => Ok(meta),
        Err(e) => {
            written.remove(dir, created);
            Err(e)
        }
    }
}

/// What a write created, removed again if it fails.
#[derive(Default)]
struct Written {
    files: Vec<PathBuf>,
    dirs: Vec<PathBuf>,
}

impl Written {
    fn remove(&self, dir: &Path, created: bool) {
        for f in &self.files {
            let _ = std::fs::remove_file(f);
        }
        for d in self.dirs.iter().rev() {
            let _ = std::fs::remove_dir(d);
        }
        if created {
            // only if empty: never removes anything this write did not put there
            let _ = std::fs::remove_dir(dir);
        }
    }
}

#[cfg(test)]
thread_local! {
    /// Tests: fail the write once this many shards are written.
    static FAIL_AFTER_SHARDS: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

fn write_shards(
    pyramid: Pyramid,
    dir: &Path,
    tiers: Vec<Tier>,
    opts: &SetOptions,
    cancel: &Cancel,
    written: &mut Written,
) -> Result<SetMeta> {
    let Pyramid {
        data_resolution,
        columns,
        vectors,
        weights,
        levels,
    } = pyramid;
    let mut levels: Vec<Option<Level>> = levels.into_iter().map(Some).collect();
    let dims: Vec<usize> = vectors.iter().map(|v| v.dim).collect();
    let mut shards = Vec::new();
    // whether any shard uses a version 4 feature
    let mut v4 = false;
    for (t, tier) in tiers.iter().enumerate() {
        cancel.check()?;
        let tier_levels: Vec<Level> = levels
            .iter_mut()
            .filter(|l| l.as_ref().is_some_and(|l| tier.holds(l.lod)))
            .filter_map(Option::take)
            .collect();
        // each level's rows by shard, each shard's in ascending cell order,
        // as tiling needs
        let owners: Vec<Owners> = tier_levels
            .iter()
            .map(|l| Owners::new(l, tier.shard_resolution))
            .collect::<Result<_>>()?;
        cancel.check()?;
        let cells = shard_cells(&owners);

        let tier_dir = dir.join(format!("tier={t}"));
        if !cells.is_empty() && !tier_dir.exists() {
            std::fs::create_dir(&tier_dir)?;
            written.dirs.push(tier_dir);
        }
        for &cell in &cells {
            cancel.check()?;
            let shard_levels: Vec<Level> = tier_levels
                .iter()
                .zip(&owners)
                .map(|(level, o)| gather(level, o.rows_of(cell), &dims))
                .collect();
            let shard = ShardMeta {
                tier: t,
                cell: crate::hex(cell),
                resolution: tier.shard_resolution,
            };
            let rel = format!("tier={t}/shard={}.parquet", shard.cell);
            let path = dir.join(&rel);
            let tile_opts = TileOptions {
                tile_size: tier.tile_size,
                row_group_size: tier.row_group_size,
                generator: opts.generator.clone(),
                metadata: opts.metadata.clone(),
                shard: Some(shard.clone()),
                ..Default::default()
            };
            let shard_pyramid = Pyramid {
                data_resolution,
                columns: columns.clone(),
                vectors: vectors.clone(),
                weights: weights.clone(),
                levels: shard_levels,
            };
            let tiled = TiledPyramid::new(shard_pyramid, &tile_opts, cancel)?;
            v4 |= tiled.meta().format_version >= 4;
            // write_parquet removes its own partial file on failure
            let summary = write_parquet(&tiled, &path, &opts.parquet, cancel)?;
            written.files.push(path.clone());
            shards.push(ShardEntry {
                tier: t,
                cell: shard.cell,
                path: Some(rel),
                rows: summary.rows,
                tiles: summary.tiles,
                bytes: summary.bytes,
                footer_bytes: PyramidFile::open(&path)?.footer_bytes(),
                bbox: Some(tiled.meta().bbox),
            });
            #[cfg(test)]
            if FAIL_AFTER_SHARDS.with(|f| f.get()) == Some(shards.len()) {
                return Err(invalid("injected failure"));
            }
        }
    }

    let boxes: Vec<Bbox> = shards
        .iter()
        .filter_map(|s| s.bbox)
        .map(|b| Bbox {
            west: b[0],
            south: b[1],
            east: b[2],
            north: b[3],
        })
        .collect();
    let parquet_index = opts.index == SetIndexFormat::Parquet;
    let meta = SetMeta {
        format: SET_FORMAT.to_string(),
        format_version: version_with(&vectors, v4 || parquet_index, SHARD_FILE_VERSION),
        generator: opts.generator.clone(),
        a5_crate: crate::A5_CRATE_VERSION.to_string(),
        a5_index: A5_INDEX.to_string(),
        data_resolution,
        columns: column_meta(&columns, &vectors),
        weights: weights.as_ref().map(Into::into),
        tiers,
        bbox: Bbox::covering(&boxes).unwrap_or(Bbox::WORLD).to_array(),
        shards,
        metadata: opts.metadata.iter().cloned().collect(),
    };
    if parquet_index {
        // written under a temporary name and renamed by write_set_table
        written.files.push(dir.join(SET_TABLE));
        write_set_table(&meta, None, dir.join(SET_TABLE), &opts.parquet)?;
        return Ok(meta);
    }
    let index = dir.join(SET_INDEX);
    let tmp = partial(&index);
    written.files.push(tmp.clone());
    std::fs::write(&tmp, meta.to_json()?)?;
    std::fs::rename(&tmp, &index)?;
    Ok(meta)
}

/// The shard cells owning rows of any of a tier's levels, ascending.
fn shard_cells(owners: &[Owners]) -> Vec<u64> {
    let mut cells: Vec<u64> = owners.iter().flat_map(|o| o.shards.iter().copied()).collect();
    cells.sort_unstable();
    cells.dedup();
    cells
}

/// The tiers in ascending LOD order, after checking that they are sound and
/// hold every level between them.
fn check_tiers(tiers: &[Tier], lods: &[i32]) -> Result<Vec<Tier>> {
    if tiers.is_empty() {
        return Err(invalid("a set needs at least one tier"));
    }
    for t in tiers {
        let [lo, hi] = t.lods;
        if lo < 0 || lo > hi {
            return Err(invalid(format!(
                "a tier's LODs run from a resolution to one at least as fine, not {lo} to {hi}"
            )));
        }
        if t.shard_resolution < 0 || t.shard_resolution > lo {
            return Err(invalid(format!(
                "the tier of LODs {lo} to {hi} has shard resolution {}; it must be from 0 to {lo}, no finer than the tier's coarsest LOD",
                t.shard_resolution
            )));
        }
        if t.tile_size == 0 {
            return Err(invalid(format!(
                "the tier of LODs {lo} to {hi} has tile size 0; tiles need room for at least one row"
            )));
        }
        if let Some(size) = t.row_group_size.filter(|&s| s < t.tile_size) {
            return Err(invalid(format!(
                "the tier of LODs {lo} to {hi} has row group size {size}, below its tile size {}",
                t.tile_size
            )));
        }
    }
    let mut sorted = tiers.to_vec();
    sorted.sort_by_key(|t| t.lods[0]);
    for w in sorted.windows(2) {
        if w[1].lods[0] <= w[0].lods[1] {
            return Err(invalid(format!(
                "tiers overlap: LODs {} to {} and {} to {}",
                w[0].lods[0], w[0].lods[1], w[1].lods[0], w[1].lods[1]
            )));
        }
    }
    for &lod in lods {
        if !sorted.iter().any(|t| t.holds(lod)) {
            return Err(invalid(format!("LOD {lod} is in no tier")));
        }
    }
    for t in &sorted {
        if !lods.iter().any(|&l| t.holds(l)) {
            return Err(invalid(format!(
                "the tier of LODs {} to {} holds none of the pyramid's levels ({lods:?})",
                t.lods[0], t.lods[1]
            )));
        }
    }
    Ok(sorted)
}

/// Create `dir`, or check that it is an empty directory, so a set never
/// mixes with files from another. Whether it was created.
fn prepare_dir(dir: &Path) -> Result<bool> {
    if dir.exists() {
        if !dir.is_dir() {
            return Err(invalid(format!("{} exists and is not a directory", dir.display())));
        }
        if std::fs::read_dir(dir)?.next().is_some() {
            return Err(invalid(format!(
                "{} already holds files; a set is written to a new or empty directory",
                dir.display()
            )));
        }
        return Ok(false);
    }
    std::fs::create_dir_all(dir)?;
    Ok(true)
}

/// A level's rows grouped by the shard owning each: shard `k` of `shards`
/// (ascending) owns `rows[offsets[k]..offsets[k + 1]]`, in ascending row
/// order, which is ascending cell order.
struct Owners {
    shards: Vec<u64>,
    offsets: Vec<usize>,
    rows: Vec<u32>,
}

impl Owners {
    /// Groups the rows by a stable counting sort on the shard's place in
    /// `shards`, holding 4 bytes a row (8 while sorting). The supercells
    /// are computed twice, once to find the few shards and once to place
    /// each row, rather than kept.
    fn new(level: &Level, shard_resolution: i32) -> Result<Self> {
        let cells = &level.cells;
        let shard = |c: u64| a5::cell_to_supercell(c, shard_resolution);
        // rows are in ascending cell order, so a chunk's shards come in runs
        let runs: Vec<Vec<u64>> = cells
            .par_chunks(1 << 16)
            .map(|chunk| {
                let mut out: Vec<u64> = Vec::new();
                for &c in chunk {
                    let s = shard(c)?;
                    if out.last() != Some(&s) {
                        out.push(s);
                    }
                }
                Ok(out)
            })
            .collect::<std::result::Result<_, String>>()
            .map_err(Error::A5)?;
        let mut shards: Vec<u64> = runs.into_iter().flatten().collect();
        shards.sort_unstable();
        shards.dedup();
        if shards.len() > u32::MAX as usize {
            return Err(invalid("more shards than a set can index"));
        }

        let mut key = vec![0u32; cells.len()];
        key.par_iter_mut()
            .zip(cells.par_iter())
            .try_for_each(|(k, &c)| {
                let s = shard(c)?;
                *k = shards.binary_search(&s).map_err(|_| "a shard was missed".to_string())? as u32;
                Ok::<(), String>(())
            })
            .map_err(Error::A5)?;
        let mut offsets = vec![0usize; shards.len() + 1];
        for &k in &key {
            offsets[k as usize + 1] += 1;
        }
        for k in 1..offsets.len() {
            offsets[k] += offsets[k - 1];
        }
        let mut next = offsets[..shards.len()].to_vec();
        let mut rows = vec![0u32; cells.len()];
        for (i, &k) in key.iter().enumerate() {
            let at = &mut next[k as usize];
            rows[*at] = i as u32;
            *at += 1;
        }
        Ok(Self { shards, offsets, rows })
    }

    /// The rows `shard` owns, ascending (none when it owns no row here).
    fn rows_of(&self, shard: u64) -> &[u32] {
        match self.shards.binary_search(&shard) {
            Ok(k) => &self.rows[self.offsets[k]..self.offsets[k + 1]],
            Err(_) => &[],
        }
    }
}

/// The rows `rows` of `level`, in order; `dims` are its vector columns'
/// elements per cell.
fn gather(level: &Level, rows: &[u32], dims: &[usize]) -> Level {
    let pick = |i: &u32| *i as usize;
    Level {
        lod: level.lod,
        cells: rows.par_iter().map(|r| level.cells[pick(r)]).collect(),
        ncell: rows.par_iter().map(|r| level.ncell[pick(r)]).collect(),
        values: level
            .values
            .iter()
            .map(|v| rows.par_iter().map(|r| v[pick(r)]).collect())
            .collect(),
        vectors: level
            .vectors
            .iter()
            .zip(dims)
            .map(|(v, &dim)| {
                let mut out = vec![0u8; rows.len() * dim];
                out.par_chunks_mut(dim).zip(rows.par_iter()).for_each(|(o, &i)| {
                    let i = i as usize;
                    o.copy_from_slice(&v[i * dim..(i + 1) * dim]);
                });
                out
            })
            .collect(),
        weights: if level.weights.is_empty() {
            Vec::new()
        } else {
            rows.par_iter().map(|r| level.weights[pick(r)]).collect()
        },
    }
}

fn partial(path: &Path) -> PathBuf {
    let mut name = path.file_name().map(|n| n.to_os_string()).unwrap_or_default();
    name.push(format!(".{}.partial", std::process::id()));
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::grid;
    use crate::{build, default_lods, Column, ColumnType, Leaves, Reducer};
    use arrow_array::cast::AsArray;

    fn leaves() -> Leaves {
        // leaves at 12 over an area that spans many shards at resolution 7
        let cells = grid(12, (-4.0, 0.5), (54.0, 57.5), 220);
        let n = cells.len();
        let v: Vec<f64> = (0..n).map(|i| ((i * 37) % 251) as f64).collect();
        let w: Vec<f64> = (0..n).map(|i| ((i * 11) % 97) as f64 / 3.0).collect();
        Leaves {
            cells,
            columns: vec![
                Column::new("r", ColumnType::UInt8, Reducer::RepChild, v),
                Column::new("m", ColumnType::Float32, Reducer::Mean, w),
            ],
            ..Default::default()
        }
    }

    fn tiers() -> Vec<Tier> {
        vec![
            Tier {
                lods: [2, 6],
                shard_resolution: 0,
                tile_size: 50,
                row_group_size: None,
            },
            Tier {
                lods: [7, 9],
                shard_resolution: 4,
                tile_size: 200,
                row_group_size: None,
            },
            Tier {
                lods: [10, 12],
                shard_resolution: 7,
                tile_size: 500,
                row_group_size: None,
            },
        ]
    }

    fn opts(tiers: Vec<Tier>) -> SetOptions {
        SetOptions {
            tiers,
            generator: "test".into(),
            metadata: vec![("app".into(), "{\"k\":1}".into())],
            parquet: ParquetOptions::default(),
            index: Default::default(),
        }
    }

    fn pyramid() -> Pyramid {
        build(leaves(), &default_lods(12), &Cancel::new()).unwrap()
    }

    #[test]
    fn default_tiers_follow_the_rule() {
        let t = default_tiers(18, &default_lods(18));
        let got: Vec<([i32; 2], i32)> = t.iter().map(|t| (t.lods, t.shard_resolution)).collect();
        assert_eq!(got, vec![([2, 12], 0), ([13, 15], 3), ([16, 18], 6)]);
        let t = default_tiers(15, &default_lods(15));
        let got: Vec<([i32; 2], i32)> = t.iter().map(|t| (t.lods, t.shard_resolution)).collect();
        assert_eq!(got, vec![([2, 12], 0), ([13, 15], 3)]);
        let t = default_tiers(10, &default_lods(10));
        let got: Vec<([i32; 2], i32)> = t.iter().map(|t| (t.lods, t.shard_resolution)).collect();
        assert_eq!(got, vec![([2, 10], 0)]);
        // a tier with none of the levels is left out
        let t = default_tiers(18, &[8, 17]);
        let got: Vec<[i32; 2]> = t.iter().map(|t| t.lods).collect();
        assert_eq!(got, vec![[8, 12], [16, 18]]);
        assert!(check_tiers(&t, &[8, 17, 18]).is_ok());
        for d in 1..=30 {
            let lods = default_lods(d);
            let mut all = lods.clone();
            all.push(d);
            assert!(
                check_tiers(&default_tiers(d, &lods), &all).is_ok(),
                "data resolution {d}"
            );
        }
    }

    #[test]
    fn every_cell_is_in_the_shard_containing_its_centre() {
        let p = pyramid();
        let dir = tempdir("owners");
        let meta = write_set(p.clone(), &dir, &opts(tiers()), &Cancel::new()).unwrap();

        assert_eq!(meta.format, SET_FORMAT);
        assert_eq!(meta.tiers, tiers());
        assert_eq!(meta.metadata.get("app").map(String::as_str), Some("{\"k\":1}"));
        for t in 0..3 {
            assert!(meta.shards.iter().any(|s| s.tier == t), "tier {t} has no shard");
        }
        assert!(meta.shards.iter().filter(|s| s.tier == 2).count() > 10);

        let mut back: BTreeMap<i32, Vec<(u64, u32, Vec<f64>)>> = BTreeMap::new();
        for s in &meta.shards {
            let path = dir.join(s.path.as_ref().unwrap());
            let f = PyramidFile::open(&path).unwrap();
            let m = &f.meta;
            assert_eq!(m.format_version, 2);
            let sm = m.shard.as_ref().unwrap();
            assert_eq!(
                (sm.tier, sm.cell.as_str(), sm.resolution),
                (s.tier, s.cell.as_str(), meta.tiers[s.tier].shard_resolution)
            );
            assert_eq!(m.data_resolution, 12);
            assert_eq!(s.bytes, std::fs::metadata(&path).unwrap().len());
            assert_eq!(s.footer_bytes, f.footer_bytes());
            assert_eq!(s.bbox, Some(m.bbox));
            assert_eq!(f.n_row_groups(), s.tiles);
            m.validate(s.tiles).unwrap();
            let shard_cell = u64::from_str_radix(&s.cell, 16).unwrap();
            let tier = &meta.tiers[s.tier];
            assert_eq!(m.lods(), (tier.lods[0]..=tier.lods[1]).collect::<Vec<_>>());
            // the shard's box holds every tile's box (no antimeridian here)
            let shard_box = Bbox::from_array(m.bbox).unwrap();
            for i in 0..f.n_row_groups() {
                let tb = m.tile_bbox(i);
                assert!(
                    tb.west >= shard_box.west
                        && tb.east <= shard_box.east
                        && tb.south >= shard_box.south
                        && tb.north <= shard_box.north,
                    "tile {i} of shard {} pokes out of its box",
                    s.cell
                );
            }
            // batches span row groups, so each row's level is its `lod`
            for batch in f.read(&(0..f.n_row_groups()).collect::<Vec<_>>()).unwrap() {
                use arrow_array::cast::AsArray;
                use arrow_array::types::{Float32Type, UInt32Type, UInt64Type, UInt8Type};
                let cells = batch.column(0).as_primitive::<UInt64Type>();
                let lods = batch.column(1).as_primitive::<UInt8Type>();
                let ncell = batch.column(2).as_primitive::<UInt32Type>();
                let r = batch.column(3).as_primitive::<UInt8Type>();
                let mm = batch.column(4).as_primitive::<Float32Type>();
                for k in 0..batch.num_rows() {
                    let c = cells.value(k);
                    assert_eq!(a5::cell_to_supercell(c, tier.shard_resolution).unwrap(), shard_cell);
                    back.entry(lods.value(k) as i32).or_default().push((
                        c,
                        ncell.value(k),
                        vec![r.value(k) as f64, mm.value(k) as f64],
                    ));
                }
            }
        }
        // the shards hold exactly the pyramid's rows, each once
        for level in &p.levels {
            let mut got = back.remove(&level.lod).unwrap();
            got.sort_by_key(|x| x.0);
            let want: Vec<(u64, u32, Vec<f64>)> = (0..level.len())
                .map(|i| {
                    (
                        level.cells[i],
                        level.ncell[i],
                        vec![level.values[0][i], level.values[1][i]],
                    )
                })
                .collect();
            assert_eq!(got, want, "LOD {}", level.lod);
        }
        assert!(back.is_empty());

        let read = SetMeta::read(&dir).unwrap();
        assert_eq!(read, meta);
        assert_eq!(read.tier_of(8), Some(1));
        assert_eq!(read.tier_of(1), None);
        let everywhere = read.select(2, None);
        assert_eq!(everywhere.len(), meta.shards.iter().filter(|s| s.tier == 2).count());
        let corner = Bbox::new(-4.0, 54.0, -3.9, 54.1).unwrap();
        let near = read.select(2, Some(&corner));
        assert!(!near.is_empty() && near.len() < everywhere.len());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn bad_tiers_and_directories_are_refused() {
        let p = pyramid();
        let bad = [
            vec![],
            vec![Tier {
                lods: [2, 12],
                shard_resolution: 3,
                tile_size: 10,
                row_group_size: None,
            }],
            vec![Tier {
                lods: [2, 12],
                shard_resolution: 0,
                tile_size: 0,
                row_group_size: None,
            }],
            vec![Tier {
                lods: [5, 2],
                shard_resolution: 0,
                tile_size: 10,
                row_group_size: None,
            }],
            vec![Tier {
                lods: [2, 12],
                shard_resolution: 0,
                tile_size: 10,
                row_group_size: Some(9),
            }],
            vec![
                Tier {
                    lods: [2, 8],
                    shard_resolution: 0,
                    tile_size: 10,
                    row_group_size: None,
                },
                Tier {
                    lods: [8, 12],
                    shard_resolution: 6,
                    tile_size: 10,
                    row_group_size: None,
                },
            ],
            vec![Tier {
                lods: [3, 12],
                shard_resolution: 0,
                tile_size: 10,
                row_group_size: None,
            }],
            vec![
                Tier {
                    lods: [0, 1],
                    shard_resolution: 0,
                    tile_size: 10,
                    row_group_size: None,
                },
                Tier {
                    lods: [2, 12],
                    shard_resolution: 0,
                    tile_size: 10,
                    row_group_size: None,
                },
            ],
        ];
        for (k, tiers) in bad.into_iter().enumerate() {
            let dir = tempdir(&format!("bad{k}"));
            assert!(
                matches!(
                    write_set(p.clone(), &dir, &opts(tiers), &Cancel::new()),
                    Err(Error::Invalid(_))
                ),
                "case {k}"
            );
            // refused before anything is written
            assert!(!dir.exists(), "case {k}");
        }
        let dir = tempdir("full");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("other"), "x").unwrap();
        assert!(matches!(
            write_set(p.clone(), &dir, &opts(tiers()), &Cancel::new()),
            Err(Error::Invalid(_))
        ));
        std::fs::remove_dir_all(&dir).unwrap();

        let stop = Cancel::new();
        stop.cancel();
        let dir = tempdir("stop");
        assert!(matches!(
            write_set(p, &dir, &opts(tiers()), &stop),
            Err(Error::Cancelled)
        ));
        // the directory it created is gone again
        assert!(!dir.exists());
    }

    #[test]
    fn a_failed_write_removes_what_it_wrote() {
        let p = pyramid();
        // into a directory the write creates: removed with the shards
        let dir = tempdir("fail-new");
        FAIL_AFTER_SHARDS.with(|f| f.set(Some(3)));
        let out = write_set(p.clone(), &dir, &opts(tiers()), &Cancel::new());
        FAIL_AFTER_SHARDS.with(|f| f.set(None));
        assert!(matches!(out, Err(Error::Invalid(_))));
        assert!(!dir.exists());

        // into an empty directory that was there: left empty, and a retry works
        let dir = tempdir("fail-existing");
        std::fs::create_dir_all(&dir).unwrap();
        FAIL_AFTER_SHARDS.with(|f| f.set(Some(5)));
        let out = write_set(p.clone(), &dir, &opts(tiers()), &Cancel::new());
        FAIL_AFTER_SHARDS.with(|f| f.set(None));
        assert!(out.is_err());
        assert!(dir.exists());
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
        let meta = write_set(p, &dir, &opts(tiers()), &Cancel::new()).unwrap();
        assert!(meta.shards.len() > 5);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn vectors_split_with_their_cells_in_version_3_sets() {
        let mut l = leaves();
        let n = l.cells.len();
        let bytes: Vec<u8> = (0..n * 3).map(|i| ((i * 2654435761usize) >> 7) as u8).collect();
        l.vectors = vec![crate::VectorColumn::new("e", ColumnType::UInt8, 3, bytes)];
        let whole = build(l, &crate::default_lods(12), &Cancel::new()).unwrap();
        let dir = tempdir("vectors");
        let meta = write_set(whole.clone(), &dir, &opts(tiers()), &Cancel::new()).unwrap();
        assert_eq!(meta.format_version, 3);
        assert_eq!(meta.columns.last().unwrap().dim, Some(3));
        // every row of every level, with its vector, is in exactly one shard
        let mut got: Vec<(u64, Vec<u8>)> = Vec::new();
        for s in &meta.shards {
            let f = PyramidFile::open(dir.join(s.path.as_ref().unwrap())).unwrap();
            assert_eq!(f.meta.format_version, 3);
            for b in f.read(&(0..f.n_tiles()).collect::<Vec<_>>()).unwrap() {
                let cells = b.column(0).as_primitive::<arrow_array::types::UInt64Type>();
                let e = b.column_by_name("e").unwrap().as_fixed_size_binary();
                for i in 0..b.num_rows() {
                    got.push((cells.value(i), e.value(i).to_vec()));
                }
            }
        }
        let mut want: Vec<(u64, Vec<u8>)> = whole
            .levels
            .iter()
            .flat_map(|l| (0..l.len()).map(move |i| (l.cells[i], l.vectors[0][i * 3..i * 3 + 3].to_vec())))
            .collect();
        got.sort();
        want.sort();
        assert_eq!(got, want);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn tiers_with_row_groups_of_tiles_make_a_version_4_set() {
        let mut t = tiers();
        for tier in &mut t {
            tier.row_group_size = Some(3 * tier.tile_size);
        }
        let whole = pyramid();
        let dir = tempdir("row-groups");
        let meta = write_set(whole.clone(), &dir, &opts(t.clone()), &Cancel::new()).unwrap();
        assert_eq!(meta.format_version, 4);
        assert_eq!(meta.tiers, t);
        let back = SetMeta::read(&dir).unwrap();
        assert_eq!(back.tiers, t);
        let mut several = 0;
        let mut rows = 0;
        for s in &meta.shards {
            let f = PyramidFile::open(dir.join(s.path.as_ref().unwrap())).unwrap();
            assert_eq!(s.tiles, f.n_tiles());
            if f.n_tiles() > f.n_row_groups() {
                several += 1;
                assert_eq!(f.meta.format_version, 4);
                assert_eq!(f.meta.row_group_size, Some(3 * f.meta.tile_size));
            }
            let batches = f.read(&(0..f.n_tiles()).collect::<Vec<_>>()).unwrap();
            assert_eq!(batches.len(), f.n_tiles());
            rows += batches.iter().map(|b| b.num_rows()).sum::<usize>();
        }
        assert!(several > 0);
        assert_eq!(rows, whole.n_rows());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn sets_and_shards_record_version_2() {
        let dir = tempdir("versions");
        let meta = write_set(pyramid(), &dir, &opts(tiers()), &Cancel::new()).unwrap();
        assert_eq!(meta.format_version, 2);
        let f = PyramidFile::open(dir.join(meta.shards[0].path.as_ref().unwrap())).unwrap();
        assert_eq!(f.meta.format_version, 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn owners_group_each_level_by_shard_in_cell_order() {
        let p = pyramid();
        for level in &p.levels {
            for res in [0, 2, 4] {
                if res > level.lod {
                    continue;
                }
                let o = Owners::new(level, res).unwrap();
                let mut seen = 0;
                for &s in &o.shards {
                    let rows = o.rows_of(s);
                    assert!(!rows.is_empty() && rows.windows(2).all(|w| w[0] < w[1]));
                    for &i in rows {
                        assert_eq!(a5::cell_to_supercell(level.cells[i as usize], res).unwrap(), s);
                    }
                    seen += rows.len();
                }
                assert_eq!(seen, level.len());
                assert!(o.shards.windows(2).all(|w| w[0] < w[1]));
                assert!(o.rows_of(0).is_empty());
            }
        }
        let owners: Vec<Owners> = p.levels.iter().map(|l| Owners::new(l, 0).unwrap()).collect();
        let mut want: Vec<u64> = owners.iter().flat_map(|o| o.shards.clone()).collect();
        want.sort_unstable();
        want.dedup();
        assert_eq!(shard_cells(&owners), want);
    }

    #[test]
    fn indexes_of_other_formats_are_refused() {
        assert!(matches!(
            SetMeta::from_json(r#"{"format":"x","format_version":2}"#),
            Err(Error::Format(_) | Error::Json(_))
        ));
        let p = pyramid();
        let dir = tempdir("newer");
        let mut meta = write_set(p, &dir, &opts(tiers()), &Cancel::new()).unwrap();
        meta.format_version = FORMAT_VERSION + 1;
        assert!(matches!(
            SetMeta::from_json(&meta.to_json().unwrap()),
            Err(Error::Format(_))
        ));
        meta.format_version = FORMAT_VERSION;
        meta.format = "a5pyramid".into();
        assert!(matches!(
            SetMeta::from_json(&meta.to_json().unwrap()),
            Err(Error::Format(_))
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn tempdir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("a5pyramid-set-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }
}
