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
use crate::meta::{column_meta, ColumnMeta, ShardMeta, A5_INDEX, FORMAT_VERSION, SHARD_FILE_VERSION};
use crate::read::PyramidFile;
use crate::tiled::{TileOptions, TiledPyramid};
use crate::write::{write_parquet, ParquetOptions};

/// File name of a set's index, in the set's directory.
pub const SET_INDEX: &str = "set.json";
/// The `format` a set's index declares.
pub const SET_FORMAT: &str = "a5pyramid-set";
/// Tile size of the tiers [`default_tiers`] makes.
pub const DEFAULT_SET_TILE_SIZE: usize = 16384;

/// One tier of a set: the LODs it holds (`lods[0]` to `lods[1]`,
/// inclusive), the resolution of the cells that key its shards, and the
/// most rows in each of its tiles.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Tier {
    pub lods: [i32; 2],
    pub shard_resolution: i32,
    pub tile_size: usize,
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
        });
        hi -= 3;
    }
    if hi >= low {
        tiers.push(Tier {
            lods: [low, hi],
            shard_resolution: (hi - 12).max(0).min(low),
            tile_size: DEFAULT_SET_TILE_SIZE,
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
}

/// The index of a set, `set.json` in its directory.
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
    /// The file, relative to the set's directory, with `/` separators.
    pub path: String,
    pub rows: u64,
    pub tiles: usize,
    /// The file's size.
    pub bytes: u64,
    /// Bytes from the end of the last column chunk to the end of the file:
    /// the Parquet metadata (the shard's index included), its 4-byte length
    /// and the closing `PAR1`. A reader that fetches the last `footer_bytes`
    /// bytes has the whole footer in one request.
    pub footer_bytes: u64,
    /// The box of every tile in the shard.
    pub bbox: [f64; 4],
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

    /// Read the index of the set at `path`: its directory, or its
    /// `set.json`.
    pub fn read(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        let file = if path.is_dir() {
            path.join(SET_INDEX)
        } else {
            path.to_path_buf()
        };
        Self::from_json(&std::fs::read_to_string(&file)?)
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
            .filter(|(_, s)| s.tier == tier)
            .filter(|(_, s)| {
                bbox.is_none_or(|b| {
                    let [west, south, east, north] = s.bbox;
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
        levels,
    } = pyramid;
    let mut levels: Vec<Option<Level>> = levels.into_iter().map(Some).collect();
    let mut shards = Vec::new();
    for (t, tier) in tiers.iter().enumerate() {
        cancel.check()?;
        let tier_levels: Vec<Level> = levels
            .iter_mut()
            .filter(|l| l.as_ref().is_some_and(|l| tier.holds(l.lod)))
            .filter_map(Option::take)
            .collect();
        // (shard cell, row) for every row of each level, in that order, so
        // a shard's rows come out in ascending cell order, as tiling needs
        let owners: Vec<Vec<(u64, u32)>> = tier_levels
            .iter()
            .map(|l| owner_rows(l, tier.shard_resolution))
            .collect::<Result<_>>()?;
        cancel.check()?;
        let cells = shard_cells(&owners);

        let tier_dir = dir.join(format!("tier={t}"));
        if !cells.is_empty() && !tier_dir.exists() {
            std::fs::create_dir(&tier_dir)?;
            written.dirs.push(tier_dir);
        }
        // every level's rows are consumed shard by shard, in shard order
        let mut next = vec![0usize; tier_levels.len()];
        for &cell in &cells {
            cancel.check()?;
            let shard_levels: Vec<Level> = tier_levels
                .iter()
                .zip(&owners)
                .zip(next.iter_mut())
                .map(|((level, rows), at)| {
                    let start = *at;
                    *at = start + rows[start..].partition_point(|&(s, _)| s <= cell);
                    gather(level, &rows[start..*at])
                })
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
                generator: opts.generator.clone(),
                metadata: opts.metadata.clone(),
                shard: Some(shard.clone()),
            };
            let shard_pyramid = Pyramid {
                data_resolution,
                columns: columns.clone(),
                levels: shard_levels,
            };
            let tiled = TiledPyramid::new(shard_pyramid, &tile_opts, cancel)?;
            // write_parquet removes its own partial file on failure
            let summary = write_parquet(&tiled, &path, &opts.parquet, cancel)?;
            written.files.push(path.clone());
            shards.push(ShardEntry {
                tier: t,
                cell: shard.cell,
                path: rel,
                rows: summary.rows,
                tiles: summary.row_groups,
                bytes: summary.bytes,
                footer_bytes: PyramidFile::open(&path)?.footer_bytes(),
                bbox: tiled.meta().bbox,
            });
            #[cfg(test)]
            if FAIL_AFTER_SHARDS.with(|f| f.get()) == Some(shards.len()) {
                return Err(invalid("injected failure"));
            }
        }
    }

    let boxes: Vec<Bbox> = shards
        .iter()
        .map(|s| Bbox {
            west: s.bbox[0],
            south: s.bbox[1],
            east: s.bbox[2],
            north: s.bbox[3],
        })
        .collect();
    let meta = SetMeta {
        format: SET_FORMAT.to_string(),
        format_version: SHARD_FILE_VERSION,
        generator: opts.generator.clone(),
        a5_crate: crate::A5_CRATE_VERSION.to_string(),
        a5_index: A5_INDEX.to_string(),
        data_resolution,
        columns: column_meta(&columns),
        tiers,
        bbox: Bbox::covering(&boxes).unwrap_or(Bbox::WORLD).to_array(),
        shards,
        metadata: opts.metadata.iter().cloned().collect(),
    };
    let index = dir.join(SET_INDEX);
    let tmp = partial(&index);
    written.files.push(tmp.clone());
    std::fs::write(&tmp, meta.to_json()?)?;
    std::fs::rename(&tmp, &index)?;
    Ok(meta)
}

/// The shard cells owning rows of a tier, ascending: each level's owners are
/// sorted, so a scan finds its few distinct cells without sorting every row
/// again.
fn shard_cells(owners: &[Vec<(u64, u32)>]) -> Vec<u64> {
    let mut cells = Vec::new();
    for rows in owners {
        let mut last = None;
        for &(s, _) in rows {
            if last != Some(s) {
                cells.push(s);
                last = Some(s);
            }
        }
    }
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

/// Each row's shard cell with the row, sorted.
fn owner_rows(level: &Level, shard_resolution: i32) -> Result<Vec<(u64, u32)>> {
    let mut rows: Vec<(u64, u32)> = level
        .cells
        .par_iter()
        .enumerate()
        .map(|(i, &c)| a5::cell_to_supercell(c, shard_resolution).map(|s| (s, i as u32)))
        .collect::<std::result::Result<_, String>>()
        .map_err(Error::A5)?;
    rows.par_sort_unstable();
    Ok(rows)
}

/// The rows `rows` of `level`, in order.
fn gather(level: &Level, rows: &[(u64, u32)]) -> Level {
    let pick = |i: &(u64, u32)| i.1 as usize;
    Level {
        lod: level.lod,
        cells: rows.par_iter().map(|r| level.cells[pick(r)]).collect(),
        ncell: rows.par_iter().map(|r| level.ncell[pick(r)]).collect(),
        values: level
            .values
            .iter()
            .map(|v| rows.par_iter().map(|r| v[pick(r)]).collect())
            .collect(),
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
            weights: None,
        }
    }

    fn tiers() -> Vec<Tier> {
        vec![
            Tier {
                lods: [2, 6],
                shard_resolution: 0,
                tile_size: 50,
            },
            Tier {
                lods: [7, 9],
                shard_resolution: 4,
                tile_size: 200,
            },
            Tier {
                lods: [10, 12],
                shard_resolution: 7,
                tile_size: 500,
            },
        ]
    }

    fn opts(tiers: Vec<Tier>) -> SetOptions {
        SetOptions {
            tiers,
            generator: "test".into(),
            metadata: vec![("app".into(), "{\"k\":1}".into())],
            parquet: ParquetOptions::default(),
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
            let path = dir.join(&s.path);
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
            assert_eq!(s.bbox, m.bbox);
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
            }],
            vec![Tier {
                lods: [2, 12],
                shard_resolution: 0,
                tile_size: 0,
            }],
            vec![Tier {
                lods: [5, 2],
                shard_resolution: 0,
                tile_size: 10,
            }],
            vec![
                Tier {
                    lods: [2, 8],
                    shard_resolution: 0,
                    tile_size: 10,
                },
                Tier {
                    lods: [8, 12],
                    shard_resolution: 6,
                    tile_size: 10,
                },
            ],
            vec![Tier {
                lods: [3, 12],
                shard_resolution: 0,
                tile_size: 10,
            }],
            vec![
                Tier {
                    lods: [0, 1],
                    shard_resolution: 0,
                    tile_size: 10,
                },
                Tier {
                    lods: [2, 12],
                    shard_resolution: 0,
                    tile_size: 10,
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
    fn sets_and_shards_record_version_2() {
        let dir = tempdir("versions");
        let meta = write_set(pyramid(), &dir, &opts(tiers()), &Cancel::new()).unwrap();
        assert_eq!(meta.format_version, 2);
        let f = PyramidFile::open(dir.join(&meta.shards[0].path)).unwrap();
        assert_eq!(f.meta.format_version, 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn shard_cells_are_the_distinct_owners() {
        let owners = vec![
            vec![(5, 0), (5, 1), (9, 2)],
            vec![],
            vec![(1, 0), (9, 1), (9, 2), (12, 3)],
        ];
        assert_eq!(shard_cells(&owners), vec![1, 5, 9, 12]);
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
