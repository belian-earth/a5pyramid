#' Write one level of precomputed rows as a pyramid file
#'
#' Writes rows that were reduced elsewhere, one level of them, as an
#' a5pyramid file: tiled, boxed and indexed as [a5_write_pyramid()] writes
#' its levels, but without building anything. Use it where each level is
#' its own file and its rows come from somewhere else, such as a partition
#' of a store whose levels are built from the partition's own leaves, or
#' coarse levels summed from such partitions' partial sums. Such files are
#' in format version 4.
#'
#' @section Shards and partial rows:
#' With `shard`, the file is a shard of a set: the index records its tier
#' and shard cell, and [a5_write_pyramid_set_table()] can list it. With
#' `owner = TRUE` the file also has an `owner` column, each row cell's
#' supercell at the shard resolution ([a5R::a5_cell_to_supercell()]). The
#' rows of such a shard may be *partial*: a cell split by the shard's edge
#' has a row in each shard holding some of its leaves, each summarising
#' those leaves alone, and `owner` names the shard containing the cell's
#' centre. A renderer draws every row; a reader that wants whole cells
#' merges a cell's rows, weighted by the weights column (or `ncell`), with
#' the column's codec.
#'
#' @inheritParams a5_write_pyramid
#' @param x A data frame (an [a5R::a5_cell] column, value columns and
#'   integer matrix columns for vectors) or Arrow data (a `uint64` cell
#'   column, fixed-size or plain lists of `int8` or `uint8` for vectors, as
#'   DuckDB's `TINYINT[]`) holding one level's rows, each cell once, at one
#'   resolution. Row order does not matter.
#' @param ... Value columns to write, as in [a5_pyramid()]. Default: every
#'   numeric column but `ncell` and the weights.
#' @param data_resolution The pyramid's leaves' resolution, at least the
#'   rows'. Default: the rows', for a file of leaves.
#' @param ncell The name of the column holding the leaves each row
#'   summarises, needed for rows coarser than the leaves. `NULL` (default)
#'   for leaves, whose file then has no `ncell` column.
#' @param weights The name of the column holding each row's summed leaf
#'   weights, stored as the weights column (see `weights_type`), or `NULL`.
#' @param weights_type The weights column's type. Default: `"uint32"`.
#' @param reducers,codecs,rep_sd How the rows were reduced, recorded in the
#'   index for readers, as in [a5_pyramid()]: not applied.
#' @param shard `NULL`, or a list of the shard's `tier` (from 0), `cell`
#'   (an [a5R::a5_cell] or 16 hex digits) and `resolution`.
#' @param owner Whether to add the `owner` column (needs `shard`).
#' @param lod_column Whether to store the `lod` column. Default `FALSE`: a
#'   file of one level needs none, the index gives the level.
#' @param tile_size,row_group_size As in [a5_write_pyramid()]. The defaults
#'   are the embeddings store's: tiles of 16,384 rows in row groups of up to
#'   131,072.
#' @param compression `"zstd"` (default), `"snappy"` or `"none"`.
#' @returns `dest`, invisibly.
#' @seealso [a5_write_pyramid_set_table()] to list such files as a set.
#' @export
#' @examples
#' library(a5R)
#' centre <- a5_lonlat_to_cell(-3.19, 55.95, resolution = 10)
#' cells <- a5_uncompact(a5_spherical_cap(centre, 3000), resolution = 15)
#' leaves <- data.frame(cell = cells, value = runif(length(cells)), n = 4L)
#'
#' # a level built elsewhere, here by a5_pyramid(), written as a file
#' p <- a5_pyramid(leaves, value, lods = 12, weights = "n", weights_type = "uint32")
#' l12 <- p[p$lod == 12, ]
#' path <- tempfile(fileext = ".parquet")
#' a5_write_pyramid_level(l12, path, value, data_resolution = 15, ncell = "ncell", weights = "n",
#'                        tile_size = 100, compression = "snappy")
#' a5_pyramid_info(path)
a5_write_pyramid_level <- function(x,
                                   dest,
                                   ...,
                                   data_resolution = NULL,
                                   ncell = NULL,
                                   weights = NULL,
                                   weights_type = "uint32",
                                   reducers = NULL,
                                   types = NULL,
                                   codecs = NULL,
                                   rep_sd = NULL,
                                   storage = NULL,
                                   shard = NULL,
                                   owner = FALSE,
                                   lod_column = FALSE,
                                   tile_size = 16384L,
                                   row_group_size = 131072L,
                                   compression = c("zstd", "snappy", "none"),
                                   metadata = NULL,
                                   threads = NULL) {
  check_string(dest)
  dir <- dirname(path.expand(dest))
  if (!dir.exists(dir)) {
    a5pyramid_abort("io", "The directory of {.arg dest} ({.path {dir}}) does not exist.")
  }
  compression <- rlang::arg_match(compression)
  tile_size <- check_count(tile_size)
  row_group_size <- check_row_group_size(row_group_size, tile_size)
  meta <- check_metadata(metadata)
  threads <- resolve_threads(threads)
  if (!rlang::is_bool(owner) || !rlang::is_bool(lod_column)) {
    a5pyramid_abort("input", "{.arg owner} and {.arg lod_column} must be {.code TRUE} or {.code FALSE}.")
  }
  shard <- check_shard(shard)
  if (owner && is.null(shard)) {
    a5pyramid_abort("input", "{.arg owner} names each row's shard, so {.arg shard} must be given.")
  }
  if (is.null(weights)) weights_type <- NULL
  # the reducers are recorded only: a mean by default, as the rows' values are
  input <- pyramid_input(x, rlang::enquos(...), NULL, "mean", reducers, weights, types, rep_sd, codecs,
                         storage = storage, weights_type = weights_type, ncell = ncell)
  res <- input$resolution
  data_resolution <- if (is.null(data_resolution)) res else check_count(data_resolution, min = 0L)
  rs_call(a5pyramid_write_level_rs(
    rows = input$leaves,
    data_resolution = data_resolution,
    dest = path.expand(dest),
    tile_size = tile_size,
    row_group_size = row_group_size,
    compression = compression,
    meta_keys = meta$keys,
    meta_values = meta$values,
    generator = paste("a5pyramid", utils::packageVersion("a5pyramid")),
    shard_tier = if (is.null(shard)) -1L else shard$tier,
    shard_cell = if (is.null(shard)) "" else shard$cell,
    shard_resolution = if (is.null(shard)) 0L else shard$resolution,
    owner = owner,
    lod_column = lod_column,
    ncell_column = !is.null(ncell),
    threads = threads
  ))
  invisible(dest)
}

#' Validate `shard`: NULL, or a list of tier, cell (a5_cell or hex) and
#' resolution, the cell at that resolution
#' @noRd
check_shard <- function(shard, call = rlang::caller_env()) {
  if (is.null(shard)) {
    return(NULL)
  }
  ok <- is.list(shard) && all(c("tier", "cell", "resolution") %in% names(shard))
  if (!ok) {
    a5pyramid_abort("input", "{.arg shard} must be a list of {.field tier}, {.field cell} and {.field resolution}.", call = call)
  }
  tier <- check_count(shard$tier, "shard$tier", min = 0L, call = call)
  resolution <- check_count(shard$resolution, "shard$resolution", min = 0L, call = call)
  cell <- shard$cell
  if (is.character(cell)) cell <- a5R::a5_cell(cell)
  if (!a5R::is_a5_cell(cell) || length(cell) != 1L || is.na(cell) ||
      a5R::a5_get_resolution(cell) != resolution) {
    a5pyramid_abort("input", "{.arg shard$cell} must be one A5 cell at {.arg shard$resolution} ({resolution}).", call = call)
  }
  list(tier = tier, cell = a5R::a5_u64_to_hex(cell), resolution = resolution)
}

#' Write a set's shard table
#'
#' Writes the index of a set whose shards were written one at a time, with
#' [a5_write_pyramid_level()] or by another writer, as a Parquet table: one
#' row per shard, the set's own fields in its footer (format version 4).
#' A reader filters the rows by tier and box with row-group statistics,
#' and the table can carry columns of its writer's beside the format's,
#' such as a year or a build name, so a store's manifest can be its set's
#' index. A shard may be listed with no file, as empty.
#'
#' @param shards A data frame with one row per shard, by `tier` and then
#'   `cell`: `tier` (from 0), `cell` (an [a5R::a5_cell] at the tier's shard
#'   resolution), `path` (the file, relative to the set's directory with
#'   `/` separators; `NA` for an empty shard), `rows`, `tiles`, `bytes`,
#'   `footer_bytes` and `xmin`, `ymin`, `xmax`, `ymax` (the box of every
#'   tile, `xmax` above 180 across the antimeridian, as [a5_pyramid_info()]
#'   gives it; `NA` for an empty shard). [a5_pyramid_info()] gives a file's
#'   values. Any other columns are written after these.
#' @param dest Path of the table to write, usually `set.parquet` in the
#'   set's directory.
#' @param template Path of one of the shards: the set's columns, weights,
#'   leaves' resolution and a5 crate are taken from its index.
#' @param tiers A data frame of the tiers, as for [a5_write_pyramid_set()].
#' @param metadata Named character vector of the set's metadata.
#' @inheritParams a5_write_pyramid
#' @returns `dest`, invisibly.
#' @seealso [a5_pyramid_set_info()] reads it back.
#' @export
#' @examples
#' library(a5R)
#' centre <- a5_lonlat_to_cell(-3.19, 55.95, resolution = 10)
#' cells <- a5_uncompact(a5_spherical_cap(centre, 3000), resolution = 15)
#' dir <- tempfile()
#' dir.create(dir)
#' key <- a5_cell_to_supercell(cells[1], resolution = 9)
#' a5_write_pyramid_level(data.frame(cell = cells, value = runif(length(cells))),
#'                        file.path(dir, "leaves.parquet"), value,
#'                        shard = list(tier = 0, cell = key, resolution = 9))
#' info <- a5_pyramid_info(file.path(dir, "leaves.parquet"))
#' shards <- data.frame(
#'   tier = 0L, path = "leaves.parquet", rows = sum(info$tiles$rows),
#'   tiles = nrow(info$tiles), bytes = info$bytes, footer_bytes = info$footer_bytes,
#'   xmin = info$bbox[["xmin"]], ymin = info$bbox[["ymin"]],
#'   xmax = info$bbox[["xmax"]], ymax = info$bbox[["ymax"]]
#' )
#' shards$cell <- key
#' tiers <- data.frame(lod_min = 15, lod_max = 15, shard_resolution = 9)
#' a5_write_pyramid_set_table(shards, file.path(dir, "set.parquet"),
#'                            template = file.path(dir, "leaves.parquet"), tiers = tiers)
#' a5_pyramid_set_info(dir)$shards
a5_write_pyramid_set_table <- function(shards,
                                       dest,
                                       template,
                                       tiers,
                                       metadata = NULL,
                                       compression = c("snappy", "zstd", "none")) {
  check_string(dest)
  template <- check_src(template, call = rlang::current_env())
  compression <- rlang::arg_match(compression)
  meta <- check_metadata(metadata)
  tiers <- check_tiers(tiers)
  need <- c("tier", "cell", "path", "rows", "tiles", "bytes", "footer_bytes", "xmin", "ymin", "xmax", "ymax")
  if (!is.data.frame(shards) || length(setdiff(need, names(shards))) > 0L) {
    a5pyramid_abort("input", "{.arg shards} must be a data frame with columns {.field {need}}.")
  }
  if (!a5R::is_a5_cell(shards$cell)) {
    a5pyramid_abort("input", "{.arg shards$cell} must be an {.cls a5_cell} column.")
  }
  num <- function(nm) {
    v <- shards[[nm]]
    if (!is.numeric(v) && !all(is.na(v))) {
      a5pyramid_abort("input", "{.arg shards}{.field ${nm}} must be numeric.")
    }
    as.double(v)
  }
  # the box as the format keeps it: west > east across the antimeridian
  east <- num("xmax")
  east <- ifelse(!is.na(east) & east > 180, east - 360, east)
  extra <- shards[setdiff(names(shards), need)]
  holder <- NULL
  if (ncol(extra) > 0L) {
    rlang::check_installed("arrow", reason = "to write a shard table's own columns")
    slot <- a5pyramid_stream_slot_rs()
    arrow::as_record_batch_reader(arrow::arrow_table(extra))$export_to_c(slot$address)
    holder <- slot$holder
  }
  path <- as.character(shards$path)
  rs_call(a5pyramid_write_set_table_rs(
    template = template,
    dest = path.expand(dest),
    tier_lod_min = tiers$lod_min,
    tier_lod_max = tiers$lod_max,
    tier_shard_resolution = tiers$shard_resolution,
    tier_tile_size = tiers$tile_size,
    tier_row_group_size = ifelse(is.na(tiers$row_group_size), 0L, tiers$row_group_size),
    shard_tier = as.integer(shards$tier),
    shard_cell = shards$cell,
    shard_path = path,
    shard_rows = num("rows"),
    shard_tiles = as.integer(shards$tiles),
    shard_bytes = num("bytes"),
    shard_footer_bytes = num("footer_bytes"),
    shard_box = list(num("xmin"), num("ymin"), east, num("ymax")),
    meta_keys = meta$keys,
    meta_values = meta$values,
    generator = paste("a5pyramid", utils::packageVersion("a5pyramid")),
    extra = holder,
    compression = compression
  ))
  invisible(dest)
}
