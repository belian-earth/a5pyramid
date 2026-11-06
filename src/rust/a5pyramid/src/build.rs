//! Level construction: group the leaves under each coarser LOD by supercell
//! and reduce every column within each group.

use std::collections::HashSet;

use rayon::prelude::*;

use crate::cancel::Cancel;
use crate::column::{Column, ColumnSpec, Reducer};
use crate::error::{invalid, Error, Result};
use crate::RESERVED_COLUMNS;

/// The data a pyramid is built from: cells at one resolution, value columns
/// aligned with them, and optional non-negative weights for means, modes
/// and the representative leaf (NaN weights count as zero).
#[derive(Clone, Debug, Default)]
pub struct Leaves {
    pub cells: Vec<u64>,
    pub columns: Vec<Column>,
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
    let weights = check_weights(weights, n)?;
    let data_resolution = check_cells(&cells)?;
    let mut lods = check_lods(lods, data_resolution)?;
    cancel.check()?;

    let (cells, mut columns, weights) = sort_leaves(cells, columns, weights)?;
    cancel.check()?;
    for col in &mut columns {
        let ty = col.spec.ty;
        col.values.par_iter_mut().for_each(|v| *v = ty.quantise(*v));
    }

    let rep = RepSpec::new(&columns);
    // finest first, so the largest groupings (and any error) come early
    lods.sort_unstable_by(|a, b| b.cmp(a));
    let mut levels = Vec::with_capacity(lods.len() + 1);
    for &lod in &lods {
        cancel.check()?;
        levels.push(build_level(&cells, &columns, weights.as_deref(), &rep, lod, cancel)?);
    }
    levels.reverse();
    let specs = columns.iter().map(|c| c.spec.clone()).collect();
    levels.push(Level {
        lod: data_resolution,
        ncell: vec![1; n],
        values: columns.into_iter().map(|c| c.values).collect(),
        cells,
    });
    Ok(Pyramid {
        data_resolution,
        columns: specs,
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

/// Cells, value columns and weights of the leaves, in one row order.
type LeafParts = (Vec<u64>, Vec<Column>, Option<Vec<f64>>);

/// Leaves in ascending cell order (input already in order is used as is),
/// with duplicates rejected.
fn sort_leaves(cells: Vec<u64>, columns: Vec<Column>, weights: Option<Vec<f64>>) -> Result<LeafParts> {
    if cells.par_windows(2).all(|w| w[0] < w[1]) {
        return Ok((cells, columns, weights));
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
    let gather = |v: &[f64]| -> Vec<f64> { order.par_iter().map(|&(_, i)| v[i as usize]).collect() };
    let sorted_cells = order.par_iter().map(|&(c, _)| c).collect();
    let columns = columns
        .into_iter()
        .map(|c| Column {
            values: gather(&c.values),
            spec: c.spec,
        })
        .collect();
    let weights = weights.map(|w| gather(&w));
    Ok((sorted_cells, columns, weights))
}

/// The `RepChild` columns and the scale (1 / standard deviation over the
/// leaves) that puts each on a common footing in the distance to the mean.
struct RepSpec {
    cols: Vec<usize>,
    scales: Vec<f64>,
}

impl RepSpec {
    fn new(columns: &[Column]) -> Self {
        let cols: Vec<usize> = columns
            .iter()
            .enumerate()
            .filter(|(_, c)| c.spec.reducer == Reducer::RepChild)
            .map(|(k, _)| k)
            .collect();
        let scales = cols.iter().map(|&k| inverse_sd(&columns[k].values)).collect();
        Self { cols, scales }
    }
}

/// 1 / the standard deviation of the non-missing values, or 1 when that is
/// zero or undefined (a constant column adds nothing to any distance).
/// Sums run over fixed chunks combined in order, so the scale, and with it
/// every choice of representative, is the same on any number of threads.
fn inverse_sd(v: &[f64]) -> f64 {
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
        1.0 / sd
    } else {
        1.0
    }
}

fn build_level(
    cells: &[u64],
    columns: &[Column],
    weights: Option<&[f64]>,
    rep: &RepSpec,
    lod: i32,
    cancel: &Cancel,
) -> Result<Level> {
    let mut pairs: Vec<(u64, u32)> = cells
        .par_iter()
        .enumerate()
        .map(|(i, &c)| a5::cell_to_supercell(c, lod).map(|p| (p, i as u32)))
        .collect::<std::result::Result<_, String>>()
        .map_err(Error::A5)?;
    cancel.check()?;
    // (parent, leaf) is a total order, so groups and the leaves within them
    // come out the same whatever the thread count
    pairs.par_sort_unstable();
    cancel.check()?;

    let mut starts = Vec::with_capacity(cells.len() / 2 + 2);
    starts.push(0usize);
    for k in 1..pairs.len() {
        if pairs[k].0 != pairs[k - 1].0 {
            starts.push(k);
        }
    }
    starts.push(pairs.len());
    let n_groups = starts.len() - 1;
    let group = |g: usize| &pairs[starts[g]..starts[g + 1]];

    let out_cells: Vec<u64> = (0..n_groups).into_par_iter().map(|g| pairs[starts[g]].0).collect();
    let ncell: Vec<u32> = (0..n_groups)
        .into_par_iter()
        .map(|g| (starts[g + 1] - starts[g]) as u32)
        .collect();

    let rep_leaf: Vec<u32> = if rep.cols.is_empty() {
        Vec::new()
    } else {
        let rep_values: Vec<&[f64]> = rep.cols.iter().map(|&k| columns[k].values.as_slice()).collect();
        (0..n_groups)
            .into_par_iter()
            .map_init(
                || vec![0.0; 2 * rep_values.len()],
                |scratch, g| representative(group(g), &rep_values, &rep.scales, weights, scratch),
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
            r => (0..n_groups)
                .into_par_iter()
                .map(|g| ty.quantise(reduce(r, group(g), v, weights)))
                .collect(),
        };
        values.push(out);
        cancel.check()?;
    }

    Ok(Level {
        lod,
        cells: out_cells,
        ncell,
        values,
    })
}

#[inline]
fn weight(weights: Option<&[f64]>, i: usize) -> f64 {
    weights.map_or(1.0, |w| w[i])
}

/// Mean, sum, min or max of the group's non-missing values.
fn reduce(reducer: Reducer, group: &[(u64, u32)], v: &[f64], weights: Option<&[f64]>) -> f64 {
    let present = group.iter().map(|&(_, i)| i as usize).filter(|&i| !v[i].is_nan());
    match reducer {
        Reducer::Mean => {
            let (mut s, mut w) = (0.0, 0.0);
            for i in present {
                let wi = weight(weights, i);
                if wi > 0.0 {
                    s += wi * v[i];
                    w += wi;
                }
            }
            if w > 0.0 {
                s / w
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

/// The leaf of `group` closest to the group's weighted mean over the
/// `RepChild` columns, each scaled to unit standard deviation. Leaves with
/// positive weight come before those without, and leaves with fewer
/// missing values before those with more; ties go to the first leaf, the
/// one with the lowest cell id.
fn representative(
    group: &[(u64, u32)],
    cols: &[&[f64]],
    scales: &[f64],
    weights: Option<&[f64]>,
    scratch: &mut [f64],
) -> u32 {
    let d = cols.len();
    let (mean, wsum) = scratch.split_at_mut(d);
    mean.fill(0.0);
    wsum.fill(0.0);
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
    }
    for k in 0..d {
        mean[k] = if wsum[k] > 0.0 { mean[k] / wsum[k] } else { f64::NAN };
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
            weights: None,
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
        let stop = Cancel::new();
        stop.cancel();
        assert!(matches!(
            build(leaves_with(cells, vec![]), &[2], &stop),
            Err(Error::Cancelled)
        ));
    }
}
