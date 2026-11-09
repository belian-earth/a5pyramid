#' @title a5pyramid: Level-of-Detail Pyramids of A5 Cells for Browser Rendering
#'
#' @description
#' a5pyramid builds level-of-detail pyramids of [A5](https://a5geo.org/)
#' cells, as created by [a5R][a5R::a5R-package] and a5px, and writes them
#' with the cells they summarise to one Parquet file laid out for browsers.
#' A web map reads the file's footer, picks the level for its zoom, and
#' fetches only the tiles in view with HTTP range requests, for example
#' with hyparquet and deck.gl. The engine is a Rust crate with no R
#' dependency, built to be published on its own.
#'
#' @section Building:
#' - [a5_pyramid()]: build the levels in memory and return them as a tibble.
#' - [a5_write_pyramid()]: build the levels and write them to Parquet, in
#'   Rust from end to end.
#' - [a5_pyramid_arrow()]: build the levels and return the tiles of the
#'   Parquet layout in memory, as an Arrow table, without a file.
#'
#' Each coarser level groups the leaves by the coarse cell containing their
#' centres ([a5R::a5_cell_to_supercell()]) and reduces each column by mean,
#' sum, minimum, maximum, mode or a representative leaf.
#'
#' @section Reading:
#' - [a5_pyramid_info()]: the file's index and layout, from its footer.
#' - [a5_read_pyramid()]: the rows of chosen levels within a bounding box,
#'   read as a renderer reads them.
#'
#' @section Threads:
#' [a5pyramid_threads()] sets how many threads builds use; the default is
#' every available CPU. Builds can be interrupted (Ctrl-C, or Esc in an
#' IDE).
#'
#' @section Errors:
#' Errors raised by a5pyramid have class `a5pyramid_error` and one of:
#' - `a5pyramid_error_input`: invalid arguments or cells, such as cells at
#'   mixed resolutions or a duplicated cell.
#' - `a5pyramid_error_io`: a file or directory that cannot be read or
#'   written.
#' - `a5pyramid_error_format`: a file that is not a pyramid, or one written
#'   for another A5 index or a newer format version.
#' - `a5pyramid_error_interrupted`: a build stopped by the user.
#' - `a5pyramid_error_internal`: a fault in a5pyramid; please report it.
#'
#' @keywords internal
"_PACKAGE"

# a5R's namespace must be loaded before a5pyramid's, whose .onLoad looks up
# a5R's C API; importing from it guarantees that.
#' @importFrom a5R a5_info
NULL
