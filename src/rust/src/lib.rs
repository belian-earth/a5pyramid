//! R bindings for the a5pyramid engine (the `a5pyramid` crate in
//! `./a5pyramid`). Nothing here is part of the engine: these modules convert
//! R vectors to and from its types, bridge cells through a5R's C API and run
//! the work off R's main thread so it can be interrupted.

use extendr_api::prelude::*;

mod a5r_api;
mod api;
mod run;

extendr_module! {
    mod a5pyramid;
    use api;
    use a5r_api;
}
