#' Build a level-of-detail pyramid of A5 cells
#'
#' `a5_pyramid()` aggregates A5 cells at one resolution, the *leaves*, to
#' coarser levels of detail (LODs) and returns every level as rows of one
#' tibble. [a5_write_pyramid()] builds the same pyramid and writes it to a
#' Parquet file laid out for browser rendering.
#'
#' @section Levels:
#' A cell at LOD `r` summarises the leaves whose centres it contains, as
#' given by [a5R::a5_cell_to_supercell()], so each level partitions the
#' leaves, and `ncell` counts the leaves in each cell. Supercells do not
#' chain: a leaf's supercell two resolutions up is often not the supercell
#' of its supercell one resolution up. Each level is therefore grouped from
#' the leaves directly, never from the level below, and matches
#' `a5px::a5_aggregate()` of the leaves to that resolution.
#'
#' @section Reducers:
#' Each value column is reduced over the leaves under each coarser cell by
#' its reducer, `aggregate` unless `reducers` names another:
#' - `"mean"`: the mean, weighted by `weights` when given. For a column of
#'   codes with a codec (see "Codecs"), the code of the mean of the decoded
#'   values.
#' - `"sum"`, `"min"`, `"max"`: the sum, minimum or maximum.
#' - `"mode"`: the most frequent value, by total weight when `weights` is
#'   given, ties going to the smallest value. For category codes.
#' - `"rep_child"`: the value of the representative leaf, the leaf closest
#'   to the cell's mean over all `"rep_child"` columns, each divided by its
#'   standard deviation so that units do not matter. Every `"rep_child"`
#'   column takes its value from the same leaf, so a coarse cell keeps a
#'   combination of values that occurs in the data, such as an RGB colour,
#'   rather than a blend. The standard deviations are computed over the
#'   leaves unless `rep_sd` fixes them, and are recorded in the index
#'   ([a5_pyramid_info()]).
#'
#' Missing values are skipped, and a cell whose leaves are all missing is
#' `NA`. The `weights` column, when selected, is summed unless `reducers`
#' says otherwise. Results do not depend on the number of threads.
#'
#' @section Vector columns:
#' A *vector column* holds a fixed number of 8-bit values per cell, such as
#' an embedding: an integer matrix column of a data frame (one row per
#' cell), or a fixed-size list of `int8` or `uint8` (or fixed-size binary,
#' read as `uint8` unless `types` says `"int8"`, as arrow reads a version-3
#' file's vectors) in Arrow data, as DuckDB writes `TINYINT[64]`. Its type
#' is `"int8"` (default for a matrix) or `"uint8"`. It is reduced by
#' `"rep_child"` whatever `aggregate` says: every element takes part in the
#' distance to the cell's mean, scaled by one standard deviation over all
#' the elements (fixed by `rep_sd` like any other column), so a coarse cell
#' holds the whole vector of one leaf. Named `"mean"` in `reducers`, it is
#' reduced element by element instead: each element of a coarse cell is the
#' (weighted) mean of its leaves' elements, of their decoded values with a
#' codec, stored as the vector's type; a cell whose leaves all have weight 0
#' takes their unweighted mean, since a vector is never missing. A mean
#' vector takes no part in choosing representatives. A file stores a vector
#' as one fixed-length binary column, so a
#' 64-element vector costs one column's metadata a tile rather than 64, and
#' one byte an element. Files with a vector column are format version 3.
#' `a5_pyramid()` and [a5_read_pyramid()] return it as an integer matrix
#' column. Vector columns come after the scalar columns, whatever the order
#' they are selected in. When a vector is the only `"rep_child"` input, its
#' standard deviation scales every term alike and cannot change the
#' representative.
#'
#' @section Column types:
#' Every level is stored with one schema, so each value column has one
#' storage type, set by `types`: `"int8"`, `"int16"`, `"int32"`, `"uint8"`,
#' `"uint16"`, `"uint32"`, `"float32"` or `"float64"`. Integer columns
#' default to `"int32"` and double columns to `"float64"`; a column of Arrow
#' data defaults to its own type (`"float32"` for `float`). Values, leaves
#' included, are stored as their type: an integer type rounds half to even
#' (as [round()] does) and clamps to its range, so the mean of a `"uint8"`
#' colour channel is a valid channel, and `"float32"` rounds to single
#' precision. `a5_pyramid()` returns the stored values. For a browser,
#' `"uint8"` suits colour channels and `"float32"` halves the size of
#' continuous values.
#'
#' @section Codecs:
#' A *codec* says how a column's stored codes map to the values they stand
#' for. With one, a `"mean"` decodes each leaf's code, takes the (weighted)
#' mean of the values and encodes it, so a coarse cell holds the code of the
#' mean of its leaves' values rather than the mean of their codes, which
#' differ for a nonlinear code. The leaves keep their codes as given. The
#' codec is recorded in the index ([a5_pyramid_info()]) for readers to
#' decode with. One codec is defined:
#' - `"aef"`: AlphaEarth Foundations embeddings, `"int8"` codes decoded as
#'   `sign(x) * (x / 127.5)^2` and encoded as
#'   `round(sign(v) * sqrt(abs(v)) * 127.5)`, rounded half away from zero and
#'   clamped to -128 to 127, as the a5embeddings store encodes its cell
#'   means. A coarse cell holds the code the store would write for the mean
#'   of its leaves' decoded embeddings; with each leaf's pixel count as
#'   `weights`, for the mean of their pixels, up to the leaves' own
#'   rounding. The mean is taken on `x * abs(x)`, the value times 127.5^2,
#'   which is exact for whole codes and weights, so a mean exactly half way
#'   between two codes rounds away from zero whatever the order of the
#'   sum.
#'
#' A codec applies to `"mean"` columns, scalar or vector, stored as its code
#' type.
#'
#' @param x A data frame with one [a5R::a5_cell] column, an [a5R::a5_cell]
#'   vector, or Arrow data with one `uint64` column of cells: an Arrow
#'   `Table`, `RecordBatch`, `RecordBatchReader`, `Dataset` or query from the
#'   arrow package. The cells must share one resolution and appear once
#'   each; rows with `NA` cells are dropped. An [a5R::a5_covering] holds
#'   cells at mixed resolutions: convert it with [a5R::a5_uncompact()]
#'   first. Arrow data is read into the engine batch by batch without
#'   passing through R, so a `Dataset` over Parquet files (from
#'   [arrow::open_dataset()]) builds without R holding the leaves.
#' @param ... <[`tidy-select`][tidyselect::language]> Value columns to carry
#'   into the pyramid. Default: every numeric column other than the cells.
#'   For an Arrow `Dataset` that includes its partitioning columns, such as
#'   `year` from `year=2025/` directories, so name the value columns there.
#' @param lods Integer vector of the coarser LODs to build, each below the
#'   leaves' resolution; the leaves are always kept. Default: every
#'   resolution from 2 up to the leaves'. Resolutions 0 and 1 hold the 12
#'   and 60 largest cells, which no map view draws.
#' @param aggregate The reducer for every value column: `"mean"` (default)
#'   or `"rep_child"`. See "Reducers".
#' @param reducers Named character vector of reducers for single columns,
#'   overriding `aggregate`, such as `c(count = "sum", class = "mode")`.
#' @param weights Name of a numeric column of non-negative leaf weights for
#'   `"mean"`, `"mode"` and `"rep_child"`, such as the `npix` column from
#'   `a5px::a5_read_raster()`. `NA` weights count as 0, and leaves with
#'   weight 0 do not count. Default `NULL`: every leaf counts once.
#' @param types Named character vector of storage types for single columns,
#'   such as `c(red = "uint8", ndvi = "float32")`. See "Column types".
#' @param rep_sd Named numeric vector of standard deviations for
#'   `"rep_child"` columns, such as `c(red = 60, green = 60)`. Default `NULL`:
#'   each column's standard deviation over the leaves, or 1 where that is 0
#'   or undefined (a constant column). That depends on
#'   every leaf, so pyramids built from different parts of a region choose
#'   representatives differently even where their cells are the same; fix
#'   the standard deviations (from a whole-region build's
#'   `a5_pyramid_info()$columns$sd`, say) for builds whose cells must
#'   agree.
#' @param codecs Named character vector of codecs for `"mean"` columns
#'   whose values are codes, such as `c(embedding = "aef")`. See "Codecs".
#'   Default `NULL`: every column stores its values.
#' @param threads Number of threads, or `NULL` (default) for
#'   [a5pyramid_threads()].
#'
#' @returns A tibble with one row per cell of every level, in ascending LOD
#'   and then cell id order:
#'   - `cell`: the cell, an [a5R::a5_cell].
#'   - `lod`: its LOD (A5 resolution), an integer.
#'   - `ncell`: the number of leaves it summarises (1 for the leaves).
#'   - the value columns, integer for the integer types up to `"uint16"`
#'     and double otherwise.
#'
#' @seealso [a5_write_pyramid()] to write the pyramid for a browser,
#'   [a5_pyramid_arrow()] for the same tiles as an Arrow table.
#' @export
#' @examples
#' library(a5R)
#' centre <- a5_lonlat_to_cell(-3.19, 55.95, resolution = 10)
#' cells <- a5_uncompact(a5_spherical_cap(centre, 3000), resolution = 15)
#' x <- data.frame(cell = cells, value = seq_along(cells))
#'
#' p <- a5_pyramid(x, lods = c(9, 11, 13))
#' p
#' table(p$lod)
#'
#' # a colour composite: coarse cells keep a real colour, not a blend
#' x$red <- sample(0:255, nrow(x), replace = TRUE)
#' x$green <- sample(0:255, nrow(x), replace = TRUE)
#' a5_pyramid(
#'   x, red, green,
#'   aggregate = "rep_child",
#'   types = c(red = "uint8", green = "uint8")
#' )
a5_pyramid <- function(x,
                       ...,
                       lods = NULL,
                       aggregate = c("mean", "rep_child"),
                       reducers = NULL,
                       weights = NULL,
                       types = NULL,
                       rep_sd = NULL,
                       codecs = NULL,
                       threads = NULL) {
  aggregate <- rlang::arg_match(aggregate)
  threads <- resolve_threads(threads)
  input <- pyramid_input(x, rlang::enquos(...), lods, aggregate, reducers, weights, types, rep_sd, codecs)
  out <- rs_call(a5pyramid_build_rs(
    leaves = input$leaves,
    lods = input$lods,
    threads = threads
  ))
  pyramid_tibble(out)
}

#' Write a level-of-detail pyramid of A5 cells to Parquet
#'
#' Builds the pyramid of [a5_pyramid()] and writes it, leaves included, to
#' one Parquet file laid out for a browser to read with HTTP range requests,
#' as with hyparquet. The build and the write run in Rust, so the levels
#' never pass through R.
#'
#' @section File layout:
#' Each level is split into tiles of at most `tile_size` rows, and each tile
#' is one Parquet row group. Levels follow each other in ascending LOD
#' order, coarsest first. A tile is a run of consecutive cell ids: A5 ids
#' follow a space-filling curve, so such a run is a compact patch of the
#' globe. A level is split down the A5 index hierarchy only where a run
#' holds more than `tile_size` cells, which makes tiles coarse where data
#' is sparse and fine where it is dense, and keeps every level sorted by
#' cell id.
#'
#' With `row_group_size`, each level is split down the hierarchy into row
#' groups of at most `row_group_size` rows instead, and each row group into
#' tiles of `tile_size` rows (the last shorter), each one data page of
#' every column. Engines that read whole row groups, such as DuckDB over
#' S3, then make fewer requests, while a browser still reads one tile at a
#' time: the file carries the Parquet page index, which locates each tile's
#' pages. Such a file is in format version 4.
#'
#' The file's key-value metadata holds a JSON index under `"a5pyramid"`:
#' the leaves' resolution, the value columns with their types and reducers,
#' each level's LOD, row count and tiles, and the bounding box of each tile,
#' computed from its cells' boundaries. A reader fetches the footer, picks
#' the level for the zoom and the tiles whose boxes meet the view, and
#' decodes only those row groups. `"a5_index"` records the A5 index
#' version of the ids, `"1.0"`, as across the a5R stack.
#'
#' The columns are `cell` (`uint64`), `lod` (`uint8`), `ncell` (`uint32`)
#' and the value columns with their types, `NA` stored as null. `cell` and
#' `ncell` are delta encoded and `lod` dictionary encoded; Parquet
#' statistics are kept for `cell` and `lod` only, so engines such as DuckDB
#' can skip row groups by LOD or cell while the footer stays small. The
#' format is specified in `FORMAT.md` in the package source
#' (`src/rust/a5pyramid/FORMAT.md`), with a reading recipe for hyparquet.
#'
#' @inheritParams a5_pyramid
#' @param dest Path of the Parquet file to write. An existing file is
#'   replaced. The file is written under a temporary name and renamed when
#'   complete, so a failed or interrupted write leaves no partial file.
#' @param tile_size The most rows in a tile. Larger tiles mean fewer
#'   requests and a smaller footer; smaller tiles mean less data fetched
#'   outside the view. Default: 4096.
#' @param row_group_size `NULL` (default) for one tile a row group, or the
#'   most rows in a row group of several tiles, at least `tile_size` (see
#'   "File layout"). The embeddings store uses 131072 with tiles of 16384.
#' @param compression `"snappy"` (default), `"zstd"` or `"none"`. hyparquet
#'   decodes snappy itself; zstd needs the `hyparquet-compressors` package
#'   in the browser.
#' @param metadata Named character vector or list of strings to add to the
#'   file's key-value metadata, such as a renderer's settings as JSON. The
#'   keys `"a5pyramid"`, `"a5_index"` and `"ARROW:schema"` are reserved.
#'
#' @returns `dest`, invisibly.
#' @seealso [a5_pyramid_info()] to inspect the file, [a5_read_pyramid()] to
#'   read levels back, [a5_pyramid_arrow()] for the same tiles in memory.
#' @export
#' @examples
#' library(a5R)
#' centre <- a5_lonlat_to_cell(-3.19, 55.95, resolution = 10)
#' cells <- a5_uncompact(a5_spherical_cap(centre, 3000), resolution = 15)
#' x <- data.frame(cell = cells, value = runif(length(cells)))
#'
#' path <- tempfile(fileext = ".parquet")
#' a5_write_pyramid(x, path, types = c(value = "float32"), tile_size = 1000)
#' a5_pyramid_info(path)
a5_write_pyramid <- function(x,
                             dest,
                             ...,
                             lods = NULL,
                             aggregate = c("mean", "rep_child"),
                             reducers = NULL,
                             weights = NULL,
                             types = NULL,
                             rep_sd = NULL,
                             codecs = NULL,
                             tile_size = 4096L,
                             row_group_size = NULL,
                             compression = c("snappy", "zstd", "none"),
                             metadata = NULL,
                             threads = NULL) {
  check_string(dest)
  dir <- dirname(path.expand(dest))
  if (!dir.exists(dir)) {
    a5pyramid_abort("io", "The directory of {.arg dest} ({.path {dir}}) does not exist.")
  }
  aggregate <- rlang::arg_match(aggregate)
  compression <- rlang::arg_match(compression)
  tile_size <- check_count(tile_size)
  row_group_size <- check_row_group_size(row_group_size, tile_size)
  meta <- check_metadata(metadata)
  threads <- resolve_threads(threads)
  input <- pyramid_input(x, rlang::enquos(...), lods, aggregate, reducers, weights, types, rep_sd, codecs)
  rs_call(a5pyramid_write_rs(
    leaves = input$leaves,
    lods = input$lods,
    dest = path.expand(dest),
    tile_size = tile_size,
    row_group_size = row_group_size,
    compression = compression,
    meta_keys = meta$keys,
    meta_values = meta$values,
    generator = paste("a5pyramid", utils::packageVersion("a5pyramid")),
    threads = threads
  ))
  invisible(dest)
}

#' Build a level-of-detail pyramid as an Arrow table
#'
#' Builds the pyramid of [a5_pyramid()] and tiles it as
#' [a5_write_pyramid()] does, but returns the tiles in memory as an Arrow
#' `Table` instead of writing a file: the layout of the Parquet file, in
#' Arrow. Use it where a file is not wanted, such as a viewer that ships the
#' tiles to a browser itself, or to hand the pyramid to other Arrow tools.
#'
#' Each record batch of the table is one tile, in the order of the file's
#' tiles: `arrow::as_record_batch_reader(tbl)$batches()[[i]]` holds the
#' rows of row group `i - 1` of the same pyramid written with
#' [a5_write_pyramid()] (one tile a row group), and the table's
#' schema metadata holds the same entries as the file's key-value
#' metadata: the index under `"a5pyramid"`, whose `tiles` and `tile_bbox`
#' count record batches here as they count tiles in the file, the A5
#' index version under
#' `"a5_index"`, and `metadata`. The columns and their types are those of
#' the file (see [a5_write_pyramid()]); [a5R::a5_from_arrow_table()]
#' converts the table to a data frame with an [a5R::a5_cell] column.
#'
#' The tiles cross from Rust through the Arrow C stream interface, so no
#' value is copied through R. `arrow::write_parquet()`, `arrow::write_feather()`
#' or `arrow::write_to_raw()` serialise the table, but only
#' [a5_write_pyramid()] writes the encodings and statistics the file format
#' specifies.
#'
#' @inheritParams a5_write_pyramid
#' @returns An Arrow `Table` with one record batch per tile.
#' @seealso [a5_write_pyramid()] for the same tiles in a Parquet file.
#' @export
#' @examplesIf requireNamespace("arrow", quietly = TRUE)
#' library(a5R)
#' centre <- a5_lonlat_to_cell(-3.19, 55.95, resolution = 10)
#' cells <- a5_uncompact(a5_spherical_cap(centre, 3000), resolution = 15)
#' x <- data.frame(cell = cells, value = runif(length(cells)))
#'
#' tbl <- a5_pyramid_arrow(x, types = c(value = "float32"), tile_size = 500)
#' tbl
#' tiles <- arrow::as_record_batch_reader(tbl)$batches()
#' length(tiles)
#' index <- tbl$metadata$a5pyramid
#'
#' a5_from_arrow_table(tbl)
a5_pyramid_arrow <- function(x,
                             ...,
                             lods = NULL,
                             aggregate = c("mean", "rep_child"),
                             reducers = NULL,
                             weights = NULL,
                             types = NULL,
                             rep_sd = NULL,
                             codecs = NULL,
                             tile_size = 4096L,
                             metadata = NULL,
                             threads = NULL) {
  rlang::check_installed("arrow", reason = "to return Arrow tables")
  aggregate <- rlang::arg_match(aggregate)
  tile_size <- check_count(tile_size)
  meta <- check_metadata(metadata)
  threads <- resolve_threads(threads)
  input <- pyramid_input(x, rlang::enquos(...), lods, aggregate, reducers, weights, types, rep_sd, codecs)
  stream <- rs_call(a5pyramid_arrow_rs(
    leaves = input$leaves,
    lods = input$lods,
    tile_size = tile_size,
    meta_keys = meta$keys,
    meta_values = meta$values,
    generator = paste("a5pyramid", utils::packageVersion("a5pyramid")),
    threads = threads
  ))
  # arrow moves the stream out of `stream$holder`, which owns it until then
  reader <- arrow::RecordBatchReader$import_from_c(stream$address)
  reader$read_table()
}

#' The rows from Rust as a tibble
#' @noRd
pyramid_tibble <- function(out) {
  cols <- c(list(cell = out$cell, lod = out$lod, ncell = out$ncell), out$values)
  tibble::new_tibble(cols, nrow = length(out$lod))
}

PYRAMID_TYPES <- c("int8", "int16", "int32", "uint8", "uint16", "uint32", "float32", "float64")
PYRAMID_REDUCERS <- c("mean", "sum", "min", "max", "mode", "rep_child")
# the codecs and the storage type of each one's codes
PYRAMID_CODECS <- c(aef = "int8")
RESERVED_COLUMNS <- c("cell", "lod", "ncell")
# the A5 index version of the ids, as across the a5R stack
A5_INDEX <- "1.0"

#' Validate the inputs shared by a5_pyramid() and a5_write_pyramid() and
#' resolve them to what the Rust entry points take.
#' @noRd
pyramid_input <- function(x, quos, lods, aggregate, reducers, weights, types,
                          rep_sd = NULL, codecs = NULL, call = rlang::caller_env()) {
  check_not_covering(x, call)
  arrow_src <- NULL
  if (is_arrow_data(x)) {
    arrow_src <- arrow_source(x, call)
    df <- arrow_src$proto
    cell_name <- arrow_src$cell
  } else if (a5R::is_a5_cell(x)) {
    if (length(quos) > 0L) {
      a5pyramid_abort("input", "{.arg ...} selects columns of a data frame, but {.arg x} is an {.cls a5_cell} vector.", call = call)
    }
    cells <- x
    df <- NULL
    cell_name <- NULL
  } else if (is.data.frame(x)) {
    cell_name <- find_cell_column(x, call)
    cells <- x[[cell_name]]
    df <- x
  } else {
    a5pyramid_abort("input", "{.arg x} must be a data frame with an {.cls a5_cell} column, an {.cls a5_cell} vector or Arrow data, not {.obj_type_friendly {x}}.", call = call)
  }

  names <- select_values(df, cell_name, quos, call)
  weights_v <- NULL
  if (!is.null(weights)) {
    weights_v <- check_weights(df, cell_name, weights, call)
  }
  dims <- vector_dims(df, names)
  reducers_v <- resolve_reducers(names, aggregate, reducers, weights, call, dims = dims)
  types_v <- resolve_types(df, names, types, call, arrow_types = arrow_src$types, dims = dims,
                           arrow_vectors = arrow_src$vectors)
  sd_v <- resolve_sd(rep_sd, names, reducers_v, call)
  codecs_v <- resolve_codecs(codecs, names, reducers_v, types_v, call)

  # the leaves, converted for the engine: from R vectors, or read from the
  # Arrow data batch by batch without passing through R
  leaves <- if (is.null(arrow_src)) {
    rs_call(a5pyramid_leaves_rs(
      cells = cells,
      values = unname(lapply(names, function(nm) vector_storage(df[[nm]]))),
      names = names,
      types = unname(types_v),
      reducers = unname(reducers_v),
      codecs = unname(codecs_v),
      weights = weights_v,
      sd = sd_v,
      dims = unname(dims)
    ), call = call)
  } else {
    slot <- a5pyramid_stream_slot_rs()
    arrow_src$reader$export_to_c(slot$address)
    rs_call(a5pyramid_leaves_arrow_rs(
      holder = slot$holder,
      cell = cell_name,
      names = names,
      types = unname(types_v),
      reducers = unname(reducers_v),
      codecs = unname(codecs_v),
      weights = if (is.null(weights)) "" else weights,
      sd = sd_v,
      dims = unname(dims)
    ), call = call)
  }
  info <- a5pyramid_leaves_info_rs(leaves)
  if (info$n == 0) {
    a5pyramid_abort("input", "{.arg x} has no cells to build a pyramid from.", call = call)
  }
  resolution <- info$resolution
  lods <- check_lods(lods, resolution, call)

  list(
    leaves = leaves,
    names = names,
    lods = lods,
    resolution = resolution
  )
}

#' A vector column's Arrow type: `list(type, dim, binary)` for a fixed-size
#' list of int8 or uint8 (`type` its element type, whatever the child's
#' nullability) or fixed-size binary (`type` "uint8" unless `types` says
#' "int8"), else `NULL`
#' @noRd
arrow_vector_type <- function(type) {
  if (inherits(type, "FixedSizeListType")) {
    elem <- type$value_type$ToString()
    if (!elem %in% c("int8", "uint8")) return(NULL)
    return(list(type = elem, dim = as.integer(type$list_size), binary = FALSE))
  }
  if (inherits(type, "FixedSizeBinary")) {
    return(list(type = "uint8", dim = as.integer(type$byte_width()), binary = TRUE))
  }
  NULL
}

#' Elements per cell of each value column: the columns of a matrix column
#' (a vector column), 0 for a scalar column
#' @noRd
vector_dims <- function(df, names) {
  vapply(names, function(nm) {
    col <- df[[nm]]
    if (is.matrix(col)) ncol(col) else 0L
  }, integer(1))
}

#' A vector column's matrix as integers for the engine (whole doubles
#' convert; the engine checks the range of the type)
#' @noRd
vector_storage <- function(col) {
  if (is.matrix(col) && is.double(col)) {
    whole <- is.na(col) | col == trunc(col)
    if (all(whole)) storage.mode(col) <- "integer"
  }
  col
}

is_arrow_data <- function(x) {
  inherits(x, c("ArrowTabular", "RecordBatchReader", "Dataset", "arrow_dplyr_query"))
}

#' Arrow types of value columns and the storage type each takes by default
#' @noRd
ARROW_VALUE_TYPES <- c(
  int8 = "int8", int16 = "int16", int32 = "int32",
  uint8 = "uint8", uint16 = "uint16", uint32 = "uint32",
  float = "float32", double = "float64"
)

#' A reader over Arrow data, its uint64 cell column, its columns' Arrow
#' types, and a zero-row data frame standing for it in column selection
#' (numeric where the column can be a value column)
#' @noRd
arrow_source <- function(x, call) {
  rlang::check_installed("arrow", reason = "to read Arrow data")
  reader <- arrow::as_record_batch_reader(x)
  schema <- reader$schema
  version <- schema$metadata[["a5_index"]]
  if (!is.null(version) && !identical(version, A5_INDEX)) {
    a5pyramid_abort("input", c(
      "{.arg x} holds ids from A5 index {version}, not {A5_INDEX}.",
      "i" = "Convert pre-1.0 ids once with {.fn a5R::a5_migrate}."
    ), call = call)
  }
  types <- vapply(schema$fields, function(f) f$type$ToString(), character(1))
  names(types) <- schema$names
  vectors <- lapply(schema$fields, function(f) arrow_vector_type(f$type))
  names(vectors) <- schema$names
  cell <- names(types)[types == "uint64"]
  if (length(cell) != 1L) {
    a5pyramid_abort("input", c(
      "Arrow data {.arg x} must have one {.cls uint64} column of cells.",
      "x" = if (length(cell) == 0L) "It has none." else "It has {length(cell)}: {.field {cell}}."
    ), call = call)
  }
  proto <- lapply(names(types), function(nm) {
    v <- vectors[[nm]]
    if (!is.null(v)) matrix(integer(), 0L, v$dim)
    else if (types[[nm]] %in% names(ARROW_VALUE_TYPES)) double()
    else character()
  })
  names(proto) <- names(types)
  proto[[cell]] <- a5R::a5_cell()
  list(
    reader = reader,
    cell = cell,
    types = types,
    vectors = vectors,
    proto = tibble::new_tibble(proto, nrow = 0L)
  )
}

check_not_covering <- function(x, call) {
  is_cov <- a5R::is_a5_covering(x) ||
    (is.data.frame(x) && !any(vapply(x, a5R::is_a5_cell, logical(1))) &&
      any(vapply(x, a5R::is_a5_covering, logical(1))))
  if (is_cov) {
    a5pyramid_abort("input", c(
      "A pyramid needs cells at one resolution, but {.arg x} holds an {.cls a5_covering}.",
      "i" = "Convert it to cells with {.code a5R::a5_uncompact(x, resolution = )}."
    ), call = call)
  }
}

find_cell_column <- function(df, call) {
  is_cell <- vapply(df, a5R::is_a5_cell, logical(1))
  if (sum(is_cell) == 0L) {
    a5pyramid_abort("input", "{.arg x} has no {.cls a5_cell} column.", call = call)
  }
  if (sum(is_cell) > 1L) {
    a5pyramid_abort("input", "{.arg x} has {sum(is_cell)} {.cls a5_cell} columns ({.field {names(df)[is_cell]}}); expected one.", call = call)
  }
  names(df)[is_cell]
}

check_lods <- function(lods, resolution, call) {
  if (is.null(lods)) {
    return(if (resolution > 2L) 2:(resolution - 1L) else integer())
  }
  ok <- is.numeric(lods) && !anyNA(lods) && all(lods == trunc(lods))
  if (!ok) {
    a5pyramid_abort("input", "{.arg lods} must be a vector of whole numbers.", call = call)
  }
  lods <- as.integer(lods)
  bad <- lods < 0L | lods >= resolution
  if (any(bad)) {
    a5pyramid_abort("input", c(
      "{.arg lods} must be resolutions below the cells' resolution, {resolution}.",
      "x" = "Not below it: {.val {lods[bad]}}."
    ), call = call)
  }
  if (anyDuplicated(lods)) {
    a5pyramid_abort("input", "{.arg lods} must not repeat a LOD.", call = call)
  }
  lods
}

is_value_column <- function(col) {
  is.numeric(col) && !is.factor(col) && !inherits(col, "integer64")
}

select_values <- function(df, cell_name, quos, call) {
  if (is.null(df)) {
    return(character())
  }
  cand <- df[setdiff(names(df), cell_name)]
  if (length(quos) == 0L) {
    names <- names(cand)[vapply(cand, is_value_column, logical(1))]
  } else {
    pos <- tidyselect::eval_select(rlang::expr(c(!!!quos)), data = cand, error_call = call)
    names <- names(pos)
    bad <- names[!vapply(cand[names], is_value_column, logical(1))]
    if (length(bad) > 0L) {
      a5pyramid_abort("input", c(
        "Value columns must be numeric.",
        "x" = "{.field {bad}} {?is/are} not."
      ), call = call)
    }
  }
  reserved <- intersect(names, RESERVED_COLUMNS)
  if (length(reserved) > 0L) {
    a5pyramid_abort("input", c(
      "A value column cannot be named {.val {reserved}}.",
      "i" = "The pyramid uses {.val {RESERVED_COLUMNS}} for its own columns; rename it or leave it out of {.arg ...}."
    ), call = call)
  }
  names
}

check_weights <- function(df, cell_name, weights, call) {
  if (is.null(df) || !rlang::is_string(weights) || !weights %in% setdiff(names(df), cell_name)) {
    a5pyramid_abort("input", "{.arg weights} must name a column of {.arg x}.", call = call)
  }
  w <- df[[weights]]
  if (!is_value_column(w)) {
    a5pyramid_abort("input", "{.arg weights} column {.field {weights}} must be numeric.", call = call)
  }
  if (any(w < 0 | is.infinite(w), na.rm = TRUE)) {
    a5pyramid_abort("input", "{.arg weights} must be finite and not negative.", call = call)
  }
  as.double(w)
}

#' A named, validated character vector of options for some value columns
#' @noRd
check_named_choice <- function(x, arg, names, choices, call) {
  if (is.null(x)) {
    return(character())
  }
  if (is.list(x)) x <- unlist(x)
  nm <- names(x)
  if (!is.character(x) || is.null(nm) || anyNA(nm) || any(!nzchar(nm)) || anyDuplicated(nm)) {
    a5pyramid_abort("input", "{.arg {arg}} must be a character vector named by value column.", call = call)
  }
  unknown <- setdiff(nm, names)
  if (length(unknown) > 0L) {
    a5pyramid_abort("input", c(
      "{.arg {arg}} names {?a column that is not a/columns that are not} value column{?s}: {.field {unknown}}.",
      "i" = "Value columns: {.field {names}}."
    ), call = call)
  }
  bad <- setdiff(unique(x), choices)
  if (length(bad) > 0L) {
    a5pyramid_abort("input", c(
      "{.arg {arg}} must be one of {.val {choices}}.",
      "x" = "Got {.val {bad}}."
    ), call = call)
  }
  x
}

resolve_reducers <- function(names, aggregate, reducers, weights, call, dims = integer()) {
  given <- check_named_choice(reducers, "reducers", names, PYRAMID_REDUCERS, call)
  out <- rep(aggregate, length(names))
  names(out) <- names
  if (!is.null(weights) && weights %in% names) out[[weights]] <- "sum"
  # vector columns are reduced by rep_child whatever the aggregate, or by
  # mean where `reducers` says so
  vec <- names[dims[names] > 0L]
  bad <- vec[vec %in% names(given) & !given[vec] %in% c("rep_child", "mean")]
  if (length(bad) > 0L) {
    a5pyramid_abort("input", c(
      "Vector column{?s} {.field {bad}} can only be reduced by {.val rep_child} or {.val mean}.",
      "i" = "A coarse cell takes the vector of one representative leaf, or the mean of its leaves' vectors."
    ), call = call)
  }
  out[vec] <- "rep_child"
  out[names(given)] <- given
  out
}

resolve_types <- function(df, names, types, call, arrow_types = NULL, dims = integer(),
                          arrow_vectors = NULL) {
  given <- check_named_choice(types, "types", names, PYRAMID_TYPES, call)
  out <- vapply(names, function(nm) {
    if (!is.null(arrow_types)) {
      v <- arrow_vectors[[nm]]
      if (!is.null(v)) v$type else ARROW_VALUE_TYPES[[arrow_types[[nm]]]]
    } else if (isTRUE(dims[nm] > 0L)) {
      "int8"
    } else if (is.integer(df[[nm]])) {
      "int32"
    } else {
      "float64"
    }
  }, character(1))
  names(out) <- names
  vec <- names[dims[names] > 0L]
  bad <- vec[vec %in% names(given) & !given[vec] %in% c("int8", "uint8")]
  if (length(bad) > 0L) {
    a5pyramid_abort("input", "Vector column{?s} {.field {bad}} must be {.val int8} or {.val uint8}.", call = call)
  }
  if (!is.null(arrow_types)) {
    # a fixed-size list's element type is real; fixed-size binary is bytes,
    # read as uint8 unless `types` says int8
    listed <- vec[!vapply(vec, function(nm) isTRUE(arrow_vectors[[nm]]$binary), logical(1))]
    fixed <- listed[listed %in% names(given) & given[listed] != out[listed]]
    if (length(fixed) > 0L) {
      a5pyramid_abort("input", "The element type of Arrow vector column{?s} {.field {fixed}} comes from the data; leave {?it/them} out of {.arg types}.", call = call)
    }
  }
  out[names(given)] <- given
  out
}

#' Each value column's codec, `""` for none: a codec applies to a
#' `"mean"` column of its code type
#' @noRd
resolve_codecs <- function(codecs, names, reducers, types, call) {
  given <- check_named_choice(codecs, "codecs", names, names(PYRAMID_CODECS), call)
  out <- rep("", length(names))
  names(out) <- names
  if (length(given) == 0L) {
    return(out)
  }
  cols <- names(given)
  not_mean <- cols[reducers[cols] != "mean"]
  if (length(not_mean) > 0L) {
    a5pyramid_abort("input", c(
      "A codec applies to {.val mean} columns, but {.field {not_mean}} {?is/are} reduced by {.val {unique(reducers[not_mean])}}.",
      "i" = "Set the reducer with {.arg reducers}, as {.code c({not_mean[1]} = \"mean\")}."
    ), call = call)
  }
  wrong <- cols[types[cols] != unname(PYRAMID_CODECS[given])]
  if (length(wrong) > 0L) {
    codec <- given[[wrong[1]]]
    a5pyramid_abort("input", c(
      "Codec {.val {codec}} decodes {.val {PYRAMID_CODECS[[codec]]}} codes, but {.field {wrong[1]}} is {.val {types[[wrong[1]]]}}.",
      "i" = "Store the codes as {.val {PYRAMID_CODECS[[codec]]}} with {.arg types}."
    ), call = call)
  }
  out[cols] <- given
  out
}

#' The standard deviation fixing each value column's rep_child scale, NA
#' where it is computed from the leaves (and for other reducers)
#' @noRd
resolve_sd <- function(rep_sd, names, reducers, call) {
  out <- rep(NA_real_, length(names))
  names(out) <- names
  if (is.null(rep_sd)) {
    return(unname(out))
  }
  if (is.list(rep_sd)) rep_sd <- unlist(rep_sd)
  nm <- names(rep_sd)
  if (!is.numeric(rep_sd) || is.null(nm) || anyNA(nm) || any(!nzchar(nm)) || anyDuplicated(nm)) {
    a5pyramid_abort("input", "{.arg rep_sd} must be a numeric vector named by {.val rep_child} column.", call = call)
  }
  rep_cols <- names[reducers == "rep_child"]
  unknown <- setdiff(nm, rep_cols)
  if (length(unknown) > 0L) {
    a5pyramid_abort("input", c(
      "{.arg rep_sd} names {?a column that is not a/columns that are not} {.val rep_child} column{?s}: {.field {unknown}}.",
      "i" = if (length(rep_cols)) "{.val rep_child} columns: {.field {rep_cols}}." else "No column is reduced by {.val rep_child}."
    ), call = call)
  }
  bad <- nm[is.na(rep_sd) | !is.finite(rep_sd) | rep_sd <= 0]
  if (length(bad) > 0L) {
    a5pyramid_abort("input", "{.arg rep_sd} must be positive and finite, not for {.field {bad}}.", call = call)
  }
  out[nm] <- as.double(rep_sd)
  unname(out)
}

check_metadata <- function(metadata, call = rlang::caller_env()) {
  if (is.null(metadata) || length(metadata) == 0L) {
    return(list(keys = character(), values = character()))
  }
  nm <- names(metadata)
  is_str <- if (is.list(metadata)) all(vapply(metadata, rlang::is_string, logical(1))) else is.character(metadata) && !anyNA(metadata)
  if (!is_str || is.null(nm) || anyNA(nm) || any(!nzchar(nm)) || anyDuplicated(nm)) {
    a5pyramid_abort("input", c(
      "{.arg metadata} must be a named character vector or a named list of strings.",
      "i" = "Encode structured values as JSON first."
    ), call = call)
  }
  reserved <- intersect(nm, c("a5pyramid", "a5_index", "ARROW:schema"))
  if (length(reserved) > 0L) {
    a5pyramid_abort("input", "{.arg metadata} key{?s} {.val {reserved}} {?is/are} reserved.", call = call)
  }
  list(keys = nm, values = unname(as.character(unlist(metadata))))
}
