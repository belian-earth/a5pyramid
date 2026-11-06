//! Bounding boxes of tiles, from the boundaries of their cells.
//!
//! A box is `[west, south, east, north]` in degrees. As in GeoJSON (RFC 7946
//! section 5.2), `west > east` means the box crosses the antimeridian. A box
//! holding a pole spans every longitude and reaches the pole.

use crate::error::{invalid, Error, Result};

/// Boxes are rounded outward to this many decimal places (about 1 m).
const DECIMALS: f64 = 1e5;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bbox {
    pub west: f64,
    pub south: f64,
    pub east: f64,
    pub north: f64,
}

impl Bbox {
    pub const WORLD: Bbox = Bbox {
        west: -180.0,
        south: -90.0,
        east: 180.0,
        north: 90.0,
    };

    /// A checked box: longitudes in -180..180, latitudes in -90..90 with
    /// `south <= north`, and `west > east` for one across the antimeridian.
    pub fn new(west: f64, south: f64, east: f64, north: f64) -> Result<Self> {
        let lon_ok = |x: f64| (-180.0..=180.0).contains(&x);
        let lat_ok = |y: f64| (-90.0..=90.0).contains(&y);
        if !(lon_ok(west) && lon_ok(east) && lat_ok(south) && lat_ok(north)) || south > north {
            return Err(invalid(format!(
                "bbox [{west}, {south}, {east}, {north}] must have longitudes in -180..180 and latitudes in -90..90 with south <= north"
            )));
        }
        Ok(Self {
            west,
            south,
            east,
            north,
        })
    }

    pub fn from_array(b: [f64; 4]) -> Result<Self> {
        Self::new(b[0], b[1], b[2], b[3])
    }

    pub fn to_array(self) -> [f64; 4] {
        [self.west, self.south, self.east, self.north]
    }

    pub fn crosses_antimeridian(&self) -> bool {
        self.west > self.east
    }

    /// The longitude range as one or two intervals within -180..180.
    fn lon_intervals(&self) -> ([f64; 2], Option<[f64; 2]>) {
        if self.crosses_antimeridian() {
            ([self.west, 180.0], Some([-180.0, self.east]))
        } else {
            ([self.west, self.east], None)
        }
    }

    /// Whether the boxes share any point (edges included).
    pub fn intersects(&self, other: &Bbox) -> bool {
        if self.south > other.north || other.south > self.north {
            return false;
        }
        let (a1, a2) = self.lon_intervals();
        let (b1, b2) = other.lon_intervals();
        let meet = |a: [f64; 2], b: [f64; 2]| a[0] <= b[1] && b[0] <= a[1];
        [Some(a1), a2]
            .into_iter()
            .flatten()
            .any(|a| [Some(b1), b2].into_iter().flatten().any(|b| meet(a, b)))
    }

    /// The smallest box covering every box in `boxes` (`None` when empty).
    pub fn covering(boxes: &[Bbox]) -> Option<Bbox> {
        let first = boxes.first()?;
        let (mut south, mut north) = (first.south, first.north);
        let mut arcs = Vec::with_capacity(boxes.len());
        for b in boxes {
            south = south.min(b.south);
            north = north.max(b.north);
            let east = if b.crosses_antimeridian() {
                b.east + 360.0
            } else {
                b.east
            };
            arcs.push((b.west, east));
        }
        Some(finish(covering_arc(arcs), south, north))
    }
}

/// Extent of one cell: its longitudes unwrapped into one continuous arc
/// `lon_lo..lon_hi`, its latitudes, and which pole (if any) it contains.
struct CellExtent {
    lon_lo: f64,
    lon_hi: f64,
    lat_lo: f64,
    lat_hi: f64,
    pole: i8,
}

/// `d` shifted by whole turns into -180..180.
fn wrap(d: f64) -> f64 {
    d - 360.0 * (d / 360.0).round()
}

fn cell_extent(cell: u64) -> Result<CellExtent> {
    let opts = a5::CellToBoundaryOptions {
        closed_ring: false,
        segments: None,
    };
    let ring = a5::cell_to_boundary(cell, Some(opts)).map_err(Error::A5)?;
    if ring.is_empty() {
        return Err(invalid(format!("cell {} has no boundary", crate::hex(cell))));
    }
    let (lon0, lat0) = ring[0].to_degrees();
    let (mut acc, mut lo, mut hi) = (lon0, lon0, lon0);
    let (mut lat_lo, mut lat_hi, mut lat_sum) = (lat0, lat0, lat0);
    let mut prev = lon0;
    let mut winding = 0.0;
    for k in 1..=ring.len() {
        let (lon, lat) = ring[k % ring.len()].to_degrees();
        let d = wrap(lon - prev);
        winding += d;
        prev = lon;
        if k < ring.len() {
            acc += d;
            lo = lo.min(acc);
            hi = hi.max(acc);
            lat_lo = lat_lo.min(lat);
            lat_hi = lat_hi.max(lat);
            lat_sum += lat;
        }
    }
    // a ring around a pole turns through a full circle of longitude
    let pole = if winding.abs() > 180.0 {
        if lat_sum > 0.0 {
            1
        } else {
            -1
        }
    } else {
        0
    };
    Ok(CellExtent {
        lon_lo: lo,
        lon_hi: hi,
        lat_lo,
        lat_hi,
        pole,
    })
}

/// The box of a set of cells: every cell's boundary lies inside it.
pub(crate) fn cells_bbox(cells: &[u64]) -> Result<Bbox> {
    let mut south = f64::INFINITY;
    let mut north = f64::NEG_INFINITY;
    let mut pole = false;
    let mut arcs = Vec::with_capacity(cells.len());
    for &c in cells {
        let e = cell_extent(c)?;
        south = south.min(e.lat_lo);
        north = north.max(e.lat_hi);
        match e.pole {
            1 => {
                north = 90.0;
                pole = true;
            }
            -1 => {
                south = -90.0;
                pole = true;
            }
            _ => {}
        }
        arcs.push((e.lon_lo, e.lon_hi));
    }
    if arcs.is_empty() {
        return Err(invalid("no cells to bound"));
    }
    let lon = if pole { None } else { covering_arc(arcs) };
    Ok(finish(lon, south, north))
}

/// The shortest arc of longitude covering every arc `(lo, hi)` (`hi >= lo`,
/// either end possibly past +-180), as `(west, east)` within -180..180 with
/// `west > east` across the antimeridian; `None` when no shorter arc than
/// the full circle covers them. The covering arc is the complement of the
/// widest gap between the arcs.
fn covering_arc(mut arcs: Vec<(f64, f64)>) -> Option<(f64, f64)> {
    for a in &mut arcs {
        let shift = 360.0 * ((a.0 + 180.0) / 360.0).floor();
        a.0 -= shift;
        a.1 -= shift;
        if a.1 - a.0 >= 360.0 {
            return None;
        }
    }
    arcs.sort_by(|a, b| a.0.total_cmp(&b.0));
    let first = arcs[0].0;
    let mut reach = arcs[0].1;
    let mut gap = (0.0, 0.0, 0.0); // (width, start, end)
    for a in &arcs[1..] {
        if a.0 - reach > gap.0 {
            gap = (a.0 - reach, reach, a.0);
        }
        reach = reach.max(a.1);
    }
    if first + 360.0 - reach > gap.0 {
        gap = (first + 360.0 - reach, reach, first + 360.0);
    }
    if gap.0 <= 0.0 {
        return None;
    }
    let west = gap.2 - 360.0 * ((gap.2 + 180.0) / 360.0).floor();
    let mut east = west + (360.0 - gap.0);
    if east > 180.0 {
        east -= 360.0;
    }
    Some((west, east))
}

/// Round outward and assemble a box from a longitude arc (`None` for all
/// longitudes) and a latitude range.
fn finish(lon: Option<(f64, f64)>, south: f64, north: f64) -> Bbox {
    let down = |x: f64| (x * DECIMALS).floor() / DECIMALS;
    let up = |x: f64| (x * DECIMALS).ceil() / DECIMALS;
    let south = down(south).max(-90.0);
    let north = up(north).min(90.0);
    let full = Bbox {
        west: -180.0,
        south,
        east: 180.0,
        north,
    };
    let Some((west, east)) = lon else { return full };
    let (west, east) = (down(west).max(-180.0), up(east).min(180.0));
    let span = if west > east { east + 360.0 - west } else { east - west };
    if span >= 360.0 || (west <= -180.0 && east >= 180.0) {
        return full;
    }
    Bbox {
        west,
        south,
        east,
        north,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::grid;

    fn ll(cell: u64) -> (f64, f64) {
        a5::cell_to_lonlat(cell).unwrap().to_degrees()
    }

    #[test]
    fn box_holds_every_vertex() {
        for res in [3, 8, 14] {
            let cells = grid(res, (8.0, 12.0), (44.0, 47.0), 80);
            let b = cells_bbox(&cells).unwrap();
            assert!(!b.crosses_antimeridian());
            for &c in &cells {
                for v in a5::cell_to_boundary(c, None).unwrap() {
                    let (x, y) = v.to_degrees();
                    assert!(
                        x >= b.west && x <= b.east && y >= b.south && y <= b.north,
                        "{x},{y} outside {b:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn antimeridian_boxes_cross() {
        let mut cells = grid(9, (179.0, 180.0), (-17.0, -16.0), 60);
        cells.extend(grid(9, (-180.0, -179.0), (-17.0, -16.0), 60));
        cells.sort_unstable();
        cells.dedup();
        let b = cells_bbox(&cells).unwrap();
        assert!(b.crosses_antimeridian(), "{b:?}");
        assert!(
            b.west > 178.0 && b.west < 179.0 && b.east > -179.0 && b.east < -178.0,
            "{b:?}"
        );
        for &c in &cells {
            let (x, y) = ll(c);
            let lon_in = x >= b.west || x <= b.east;
            assert!(lon_in && y >= b.south && y <= b.north);
        }
        let fiji = Bbox::new(179.5, -16.8, -179.5, -16.2).unwrap();
        assert!(b.intersects(&fiji));
        assert!(!b.intersects(&Bbox::new(0.0, -17.0, 10.0, -16.0).unwrap()));
    }

    #[test]
    fn polar_cells() {
        // the resolution-0 face around each pole encloses it
        for lat in [90.0, -90.0] {
            let face = a5::lonlat_to_cell(a5::LonLat::new(0.0, lat), 0).unwrap();
            let b = cells_bbox(&[face]).unwrap();
            assert_eq!((b.west, b.east), (-180.0, 180.0));
            assert_eq!(if lat > 0.0 { b.north } else { b.south }, lat);
        }
        // finer cells meet at the pole as wedges, with the pole a vertex
        for res in [2, 4, 9] {
            let wedge = a5::lonlat_to_cell(a5::LonLat::new(0.0, 90.0), res).unwrap();
            let b = cells_bbox(&[wedge]).unwrap();
            assert_eq!(b.north, 90.0);
            assert!(b.west > -180.0 || b.east < 180.0, "{b:?}");
            for v in a5::cell_to_boundary(wedge, None).unwrap() {
                let (x, y) = v.to_degrees();
                if y < 90.0 {
                    let lon_in = if b.crosses_antimeridian() {
                        x >= b.west || x <= b.east
                    } else {
                        x >= b.west && x <= b.east
                    };
                    assert!(lon_in && y >= b.south, "{x},{y} outside {b:?}");
                }
            }
        }
    }

    #[test]
    fn covering_arcs() {
        assert_eq!(covering_arc(vec![(10.0, 20.0), (15.0, 30.0)]), Some((10.0, 30.0)));
        assert_eq!(
            covering_arc(vec![(170.0, 175.0), (-175.0, -170.0)]),
            Some((170.0, -170.0))
        );
        // an arc already unwrapped past 180
        assert_eq!(covering_arc(vec![(178.0, 182.0)]), Some((178.0, -178.0)));
        assert_eq!(covering_arc(vec![(-180.0, 0.0), (0.0, 180.0)]), None);
        let world = Bbox::covering(&[Bbox::WORLD]).unwrap();
        assert_eq!(world, Bbox::WORLD);
        let two = Bbox::covering(&[
            Bbox::new(170.0, 0.0, -170.0, 1.0).unwrap(),
            Bbox::new(-175.0, -1.0, -160.0, 0.5).unwrap(),
        ])
        .unwrap();
        assert_eq!(two.to_array(), [170.0, -1.0, -160.0, 1.0]);
    }

    #[test]
    fn checked_boxes() {
        assert!(Bbox::new(-181.0, 0.0, 0.0, 1.0).is_err());
        assert!(Bbox::new(0.0, 2.0, 1.0, 1.0).is_err());
        assert!(Bbox::new(10.0, 0.0, -10.0, 1.0).unwrap().crosses_antimeridian());
    }
}
