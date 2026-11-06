//! a5R's C API for `a5_cell` vectors (signatures in a5R's
//! `inst/include/a5R.h`).
//!
//! Cells cross between the packages as 64-bit ids through a5R's own
//! functions, so a change to a5R's internal layout of `a5_cell` cannot give
//! wrong ids silently. The functions are looked up once with
//! `R_GetCCallable`, which needs a5R's namespace loaded; importing a5R
//! guarantees it before a5pyramid loads. Every call must be made on R's
//! main thread, as for any R API.

use std::ffi::{c_char, c_int, c_void};
use std::sync::OnceLock;

use extendr_api::prelude::*;
use extendr_api::SEXP;

/// R's `R_xlen_t` (`ptrdiff_t`).
type RXlen = isize;

extern "C" {
    fn R_GetCCallable(package: *const c_char, name: *const c_char) -> Option<unsafe extern "C" fn() -> *mut c_void>;
}

struct Api {
    version: unsafe extern "C" fn() -> c_int,
    from_u64: unsafe extern "C" fn(*const u64, *const c_int, RXlen) -> SEXP,
    to_u64: unsafe extern "C" fn(SEXP, *mut u64, *mut c_int) -> RXlen,
}

static API: OnceLock<Api> = OnceLock::new();

fn api() -> &'static Api {
    API.get_or_init(|| unsafe {
        type Raw = unsafe extern "C" fn() -> *mut c_void;
        let get = |name: &std::ffi::CStr| {
            R_GetCCallable(c"a5R".as_ptr(), name.as_ptr()).expect("a5R registers its C API (a5R >= 0.9.9.9003)")
        };
        Api {
            version: std::mem::transmute::<Raw, unsafe extern "C" fn() -> c_int>(get(c"a5R_api_version")),
            from_u64: std::mem::transmute::<Raw, unsafe extern "C" fn(*const u64, *const c_int, RXlen) -> SEXP>(get(
                c"a5R_cells_from_u64",
            )),
            to_u64: std::mem::transmute::<Raw, unsafe extern "C" fn(SEXP, *mut u64, *mut c_int) -> RXlen>(get(
                c"a5R_cells_to_u64",
            )),
        }
    })
}

/// The version of a5R's C API.
pub(crate) fn version() -> i32 {
    unsafe { (api().version)() }
}

/// The ids of an `a5_cell` vector and whether each is NA (id 0 there).
pub(crate) fn cells_to_u64(cells: &Robj) -> extendr_api::Result<(Vec<u64>, Vec<bool>)> {
    if !cells.inherits("a5_cell") {
        return Err(Error::Other("expected an a5_cell vector".into()));
    }
    let api = api();
    unsafe {
        let sexp = cells.get();
        let n = (api.to_u64)(sexp, std::ptr::null_mut(), std::ptr::null_mut());
        let n = usize::try_from(n).map_err(|_| Error::Other("negative a5_cell length".into()))?;
        let mut ids = vec![0u64; n];
        let mut is_na = vec![0 as c_int; n];
        (api.to_u64)(sexp, ids.as_mut_ptr(), is_na.as_mut_ptr());
        Ok((ids, is_na.into_iter().map(|x| x != 0).collect()))
    }
}

/// An `a5_cell` vector of `ids`.
pub(crate) fn cells_from_u64(ids: &[u64]) -> Robj {
    unsafe { Robj::from_sexp((api().from_u64)(ids.as_ptr(), std::ptr::null(), ids.len() as RXlen)) }
}

/// The version of a5R's C API, checked when a5pyramid loads.
/// @noRd
/// @keywords internal
#[extendr]
fn a5pyramid_a5r_api_version_rs() -> i32 {
    version()
}

/// The a5 crate version a5pyramid is built with.
/// @noRd
/// @keywords internal
#[extendr]
fn a5pyramid_a5_crate_version_rs() -> &'static str {
    a5pyramid::A5_CRATE_VERSION
}

/// Cells of lon/lat points at `resolution` by a5pyramid's own a5 crate, for
/// the load-time check that a5pyramid and a5R agree on cell ids.
/// @noRd
/// @keywords internal
#[extendr]
fn a5pyramid_lonlat_to_cells_rs(lon: &[f64], lat: &[f64], resolution: i32) -> extendr_api::Result<Robj> {
    let ids = lon
        .iter()
        .zip(lat)
        .map(|(&x, &y)| a5::lonlat_to_cell(a5::LonLat::new(x, y), resolution))
        .collect::<std::result::Result<Vec<u64>, String>>()
        .map_err(|e| Error::Other(format!("a5 error: {e}")))?;
    Ok(cells_from_u64(&ids))
}

/// Supercells of `cells` at `resolution` by a5pyramid's own a5 crate, for
/// the load-time check that both packages group cells alike.
/// @noRd
/// @keywords internal
#[extendr]
fn a5pyramid_supercells_rs(cells: Robj, resolution: i32) -> extendr_api::Result<Robj> {
    let (ids, _) = cells_to_u64(&cells)?;
    let out = ids
        .iter()
        .map(|&c| a5::cell_to_supercell(c, resolution))
        .collect::<std::result::Result<Vec<u64>, String>>()
        .map_err(|e| Error::Other(format!("a5 error: {e}")))?;
    Ok(cells_from_u64(&out))
}

extendr_module! {
    mod a5r_api;
    fn a5pyramid_a5r_api_version_rs;
    fn a5pyramid_a5_crate_version_rs;
    fn a5pyramid_lonlat_to_cells_rs;
    fn a5pyramid_supercells_rs;
}
