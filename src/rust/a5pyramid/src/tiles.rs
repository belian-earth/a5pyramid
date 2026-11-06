//! Tiling a level into row groups.
//!
//! A level's cells are sorted by id, and A5 ids follow a space-filling
//! curve: the descendants of a cell in the index hierarchy
//! ([`a5::cell_to_parent`]) are a contiguous run of ids and a compact patch
//! of the globe. A tile is such a run. The level is split down the
//! hierarchy only where a run holds more than `max_rows` cells, so tiles
//! are coarse where data is sparse and fine where it is dense, and
//! neighbouring small tiles under one parent are merged up to `max_rows`.
//! Runs never merge across the 12 resolution-0 cells, whose ids jump
//! between faces of the dodecahedron.
//!
//! Index parents do not contain their children exactly (a child can poke
//! out of its parent), which only blurs tile edges: tiles are bounded by
//! their cells' own boundaries (see `bbox`), not by a parent's.

use std::ops::Range;

use crate::error::{invalid, Error, Result};

/// Partition sorted, unique `cells` (one resolution) into contiguous tiles
/// of at most `max_rows` cells, in id order.
pub fn plan_tiles(cells: &[u64], max_rows: usize) -> Result<Vec<Range<usize>>> {
    if max_rows == 0 {
        return Err(invalid("tiles need room for at least one row"));
    }
    if cells.is_empty() {
        return Ok(Vec::new());
    }
    let res = a5::get_resolution(cells[0]);
    let mut out = Vec::new();
    for face in runs(cells, 0..cells.len(), 0)? {
        split(cells, face, 0, res, max_rows, &mut out)?;
    }
    Ok(out)
}

fn ancestor(cell: u64, t: i32) -> Result<u64> {
    a5::cell_to_parent(cell, Some(t)).map_err(Error::A5)
}

/// Runs of `range` sharing an ancestor at resolution `t`. Ancestor ids rise
/// with descendant ids, so each run ends at a partition point.
fn runs(cells: &[u64], range: Range<usize>, t: i32) -> Result<Vec<Range<usize>>> {
    let mut out = Vec::new();
    let mut s = range.start;
    while s < range.end {
        let a = ancestor(cells[s], t)?;
        let mut err = None;
        let len = cells[s..range.end].partition_point(|&c| match ancestor(c, t) {
            Ok(p) => p <= a,
            Err(e) => {
                err = Some(e);
                false
            }
        });
        if let Some(e) = err {
            return Err(e);
        }
        let e = s + len.max(1);
        out.push(s..e);
        s = e;
    }
    Ok(out)
}

/// Tiles of `range`, whose cells share an ancestor at resolution `t`.
fn split(
    cells: &[u64],
    range: Range<usize>,
    t: i32,
    res: i32,
    max_rows: usize,
    out: &mut Vec<Range<usize>>,
) -> Result<()> {
    if range.len() <= max_rows || t >= res {
        out.push(range);
        return Ok(());
    }
    let mut tiles = Vec::new();
    for child in runs(cells, range, t + 1)? {
        split(cells, child, t + 1, res, max_rows, &mut tiles)?;
    }
    merge(tiles, max_rows, out);
    Ok(())
}

/// Join neighbouring tiles while the result fits in `max_rows`.
fn merge(tiles: Vec<Range<usize>>, max_rows: usize, out: &mut Vec<Range<usize>>) {
    let mut cur: Option<Range<usize>> = None;
    for t in tiles {
        match cur.as_mut() {
            Some(c) if c.end == t.start && c.len() + t.len() <= max_rows => c.end = t.end,
            _ => {
                if let Some(c) = cur.replace(t) {
                    out.push(c);
                }
            }
        }
    }
    out.extend(cur);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::grid;

    fn check_partition(tiles: &[Range<usize>], n: usize, max_rows: usize) {
        assert_eq!(tiles.first().unwrap().start, 0);
        assert_eq!(tiles.last().unwrap().end, n);
        for w in tiles.windows(2) {
            assert_eq!(w[0].end, w[1].start);
        }
        assert!(tiles.iter().all(|t| !t.is_empty() && t.len() <= max_rows));
    }

    #[test]
    fn tiles_partition_dense_cells() {
        let cells = grid(13, (10.0, 11.0), (50.0, 50.6), 500);
        let tiles = plan_tiles(&cells, 4096).unwrap();
        check_partition(&tiles, cells.len(), 4096);
        // dense data: most tiles are well filled
        let mean = cells.len() as f64 / tiles.len() as f64;
        assert!(mean > 1024.0, "mean tile size {mean}");
    }

    #[test]
    fn ancestor_runs_are_contiguous() {
        let cells = grid(12, (-1.0, 1.0), (-1.0, 1.0), 300);
        for t in [0, 1, 4, 8] {
            let runs = runs(&cells, 0..cells.len(), t).unwrap();
            let ancestors: Vec<u64> = runs.iter().map(|r| ancestor(cells[r.start], t).unwrap()).collect();
            for (r, a) in runs.iter().zip(&ancestors) {
                assert!(cells[r.clone()].iter().all(|&c| ancestor(c, t).unwrap() == *a));
            }
            let mut uniq = ancestors.clone();
            uniq.dedup();
            assert_eq!(uniq.len(), ancestors.len());
        }
    }

    #[test]
    fn sparse_cells_make_few_tiles() {
        // scattered points over a continent at a fine resolution
        let mut cells: Vec<u64> = (0..5000)
            .map(|i| {
                let x = -10.0 + 40.0 * ((i * 7919) % 5000) as f64 / 5000.0;
                let y = 35.0 + 25.0 * ((i * 104729) % 5000) as f64 / 5000.0;
                a5::lonlat_to_cell(a5::LonLat::new(x, y), 20).unwrap()
            })
            .collect();
        cells.sort_unstable();
        cells.dedup();
        let tiles = plan_tiles(&cells, 4096).unwrap();
        check_partition(&tiles, cells.len(), 4096);
        // tiles never span two resolution-0 cells, but need not be finer
        let mut faces: Vec<u64> = cells.iter().map(|&c| ancestor(c, 0).unwrap()).collect();
        faces.dedup();
        assert!(
            tiles.len() <= 2 * faces.len(),
            "{} tiles for {} faces",
            tiles.len(),
            faces.len()
        );
    }

    #[test]
    fn small_levels_are_one_tile() {
        // one tile per resolution-0 cell the level touches
        let cells = grid(5, (0.0, 10.0), (0.0, 10.0), 50);
        let faces = runs(&cells, 0..cells.len(), 0).unwrap();
        assert_eq!(plan_tiles(&cells, 4096).unwrap(), faces);
        let cells = grid(5, (12.0, 14.0), (45.0, 47.0), 30);
        assert_eq!(plan_tiles(&cells, 4096).unwrap(), vec![0..cells.len()]);
        assert!(plan_tiles(&cells, 0).is_err());
        assert!(plan_tiles(&[], 10).unwrap().is_empty());
    }

    #[test]
    fn one_row_tiles_when_asked() {
        let cells = grid(6, (0.0, 3.0), (0.0, 3.0), 40);
        let tiles = plan_tiles(&cells, 1).unwrap();
        assert_eq!(tiles.len(), cells.len());
    }
}
