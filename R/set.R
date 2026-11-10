#' Write a pyramid as a set of shard files
#'
#' Builds the pyramid of [a5_pyramid()] and writes it as a *pyramid set*: a
#' directory of small pyramid files, *shards*, and an index, `set.json`. Use
#' it where one file would be too large to open in a browser. A browser
#' reads a file's whole footer before it draws anything, and the footer
#' grows by a few hundred bytes a tile, so a single file of 150 million
#' leaves has a footer of about 20 MB. In a set, a view opens only the few
#' shards around it.
#'
#' @section Tiers and shards:
#' The LODs are split into *tiers*, each a range of LODs with a *shard
#' resolution*. A cell of a tier belongs to the shard keyed by the cell at
#' the shard resolution containing its centre
#' ([a5R::a5_cell_to_supercell()]), so every cell is in exactly one shard.
#' Each shard is one file holding its tier's LODs for the cells it owns, at
#' `tier=<t>/shard=<cell>.parquet`. The levels are built from all the
#' leaves before the pyramid is split, so a coarse cell near a shard's edge
#' summarises every leaf under it, whichever shard holds them.
#'
#' [a5_pyramid_set_tiers()] gives the default tiers: three LODs a tier from
#' the leaves down, each keyed 12 resolutions coarser than its finest LOD,
#' and the remaining LODs in one tier keyed at resolution 0. A shard full of
#' data then holds about 22 million rows, and a map view meets at most a
#' few shards of the tier it draws at any zoom.
#'
#' @section Files:
#' A shard is an a5pyramid file in format version 2: as a file from
#' [a5_write_pyramid()], with the index also recording the shard, and with
#' `data_resolution` the leaves' resolution even in shards of coarser
#' tiers. [a5_pyramid_info()] and [a5_read_pyramid()] read shards. A shard
#' can hold no cells at some LOD of its tier, never at all of them; shards
#' without cells are not written. `set.json` lists the tiers and every
#' shard with its path, rows, tiles, size, footer size and bounding box,
#' and is written last, so a set with an index is complete. A write that
#' fails or is interrupted removes the shards it wrote, and `dest` if it
#' created it, so it can be run again. The format is specified in
#' `FORMAT.md` in the package source.
#'
#' @inheritParams a5_write_pyramid
#' @param dest Directory to write the set to. It must not exist or be empty,
#'   so a set never mixes with another's files.
#' @param lods Integer vector of the coarser LODs to build. Default: every
#'   LOD the `tiers` hold below the leaves, or, without `tiers`, every
#'   resolution from 2 up to the leaves'.
#' @param tiers A data frame with one row per tier and columns `lod_min` and
#'   `lod_max` (the tier's LODs, inclusive), `shard_resolution` (from 0 up
#'   to `lod_min`) and optionally `tile_size` (default 16384). Between them
#'   the tiers must hold every level, the leaves included, and each must
#'   hold at least one. Default: [a5_pyramid_set_tiers()].
#' @returns `dest`, invisibly.
#' @seealso [a5_pyramid_set_info()] to read the index, [a5_write_pyramid()]
#'   for a single file.
#' @export
#' @examples
#' library(a5R)
#' centre <- a5_lonlat_to_cell(-3.19, 55.95, resolution = 10)
#' cells <- a5_uncompact(a5_spherical_cap(centre, 3000), resolution = 15)
#' x <- data.frame(cell = cells, value = runif(length(cells)))
#'
#' dest <- tempfile()
#' tiers <- data.frame(
#'   lod_min = c(2, 13), lod_max = c(12, 15),
#'   shard_resolution = c(0, 10), tile_size = c(4096, 4096)
#' )
#' a5_write_pyramid_set(x, dest, tiers = tiers)
#' info <- a5_pyramid_set_info(dest)
#' info
#' info$shards
a5_write_pyramid_set <- function(x,
                                 dest,
                                 ...,
                                 lods = NULL,
                                 tiers = NULL,
                                 aggregate = c("mean", "rep_child"),
                                 reducers = NULL,
                                 weights = NULL,
                                 types = NULL,
                                 rep_sd = NULL,
                                 compression = c("snappy", "zstd", "none"),
                                 metadata = NULL,
                                 threads = NULL) {
  check_string(dest)
  dest <- path.expand(dest)
  if (file.exists(dest) && !dir.exists(dest)) {
    a5pyramid_abort("io", "{.arg dest} ({.path {dest}}) is a file, not a directory.")
  }
  if (dir.exists(dest) && length(list.files(dest, all.files = TRUE, no.. = TRUE)) > 0L) {
    a5pyramid_abort("io", c(
      "{.arg dest} ({.path {dest}}) already holds files.",
      "i" = "A set is written to a new or empty directory."
    ))
  }
  aggregate <- rlang::arg_match(aggregate)
  compression <- rlang::arg_match(compression)
  meta <- check_metadata(metadata)
  tiers <- check_tiers(tiers)
  threads <- resolve_threads(threads)
  input <- pyramid_input(x, rlang::enquos(...), lods, aggregate, reducers, weights, types, rep_sd)
  if (is.null(tiers)) {
    tiers <- a5_pyramid_set_tiers(input$resolution, input$lods)
  } else if (is.null(lods)) {
    input$lods <- tier_lods(tiers, input$resolution)
  }
  rs_call(a5pyramid_write_set_rs(
    leaves = input$leaves,
    lods = input$lods,
    dest = dest,
    tier_lod_min = tiers$lod_min,
    tier_lod_max = tiers$lod_max,
    tier_shard_resolution = tiers$shard_resolution,
    tier_tile_size = tiers$tile_size,
    compression = compression,
    meta_keys = meta$keys,
    meta_values = meta$values,
    generator = paste("a5pyramid", utils::packageVersion("a5pyramid")),
    threads = threads
  ))
  invisible(dest)
}

#' Default tiers of a pyramid set
#'
#' The tiers [a5_write_pyramid_set()] uses when none are given: from the
#' leaves down, three LODs a tier, each with shard resolution 12 below its
#' finest LOD, and every remaining LOD in one tier with shard resolution 0.
#' Tiers that would hold none of the levels are left out. For leaves at
#' resolution 18 the tiers are LODs 16 to 18 (shard resolution 6), 13 to 15
#' (3) and 2 to 12 (0).
#'
#' @param data_resolution The leaves' resolution.
#' @param lods The coarser LODs. Default: every resolution from 2 up to
#'   `data_resolution`.
#' @returns A data frame with one row per tier, in ascending LOD order:
#'   `lod_min`, `lod_max`, `shard_resolution` and `tile_size`.
#' @export
#' @examples
#' a5_pyramid_set_tiers(18)
#' a5_pyramid_set_tiers(16, lods = c(4, 8, 12, 14))
a5_pyramid_set_tiers <- function(data_resolution, lods = NULL) {
  data_resolution <- check_count(data_resolution, min = 0L)
  if (data_resolution > 30L) {
    a5pyramid_abort("input", "{.arg data_resolution} must be an A5 resolution, from 0 to 30.")
  }
  lods <- check_lods(lods, data_resolution, rlang::current_env())
  as.data.frame(a5pyramid_default_tiers_rs(data_resolution, lods))
}

#' Inspect a pyramid set
#'
#' Reads the index of a set written by [a5_write_pyramid_set()],
#' `set.json`, without opening any shard. A renderer reads it first, then
#' the footers of the shards its view meets.
#'
#' @param src The set's directory, or its `set.json`.
#' @returns A list of class `a5pyramid_set_info`:
#'   - `path`: the set's directory.
#'   - `format_version`, `generator`, `a5_crate` and `a5_index`, as in
#'     [a5_pyramid_info()].
#'   - `data_resolution`: the leaves' resolution.
#'   - `bbox`: `c(xmin, ymin, xmax, ymax)` of every shard.
#'   - `columns`: a tibble of the value columns' `name`, `type`,
#'     `reducer` and `sd`, the standard deviation that scaled a
#'     `"rep_child"` column (1 where the leaves' standard deviation is 0 or
#'     undefined; `NA` for other reducers and older files), and `dim`, the
#'     elements per cell of a vector column (`NA` for scalar columns).
#'   - `tiers`: a tibble with one row per tier: `tier` (from 0), `lod_min`,
#'     `lod_max`, `shard_resolution`, `tile_size`, and its `shards`, `rows`
#'     and `bytes`.
#'   - `shards`: a tibble with one row per shard: `tier`, `cell` (the shard
#'     cell, an [a5R::a5_cell]), `path` (relative to the set's directory),
#'     `rows`, `tiles`, `bytes`, `footer_bytes` (what a reader fetches to
#'     open it) and `xmin`, `ymin`, `xmax`, `ymax`.
#'   - `metadata`: the set's metadata, a named character vector.
#' @seealso [a5_pyramid_info()] for one shard.
#' @export
#' @examples
#' library(a5R)
#' centre <- a5_lonlat_to_cell(-3.19, 55.95, resolution = 10)
#' cells <- a5_uncompact(a5_spherical_cap(centre, 3000), resolution = 15)
#' dest <- tempfile()
#' a5_write_pyramid_set(data.frame(cell = cells, value = runif(length(cells))), dest)
#' a5_pyramid_set_info(dest)$tiers
a5_pyramid_set_info <- function(src) {
  check_string(src, "src")
  src <- path.expand(src)
  dir <- if (dir.exists(src)) src else dirname(src)
  if (!file.exists(src) || (dir.exists(src) && !file.exists(file.path(src, "set.json")))) {
    a5pyramid_abort("io", "{.arg src} ({.path {src}}) is not a pyramid set: no {.file set.json}.")
  }
  r <- rs_call(a5pyramid_set_info_rs(src))
  check_index(r$a5_index, src)
  s <- r$shards
  lon <- stack_lon(s$west, s$east)
  shards <- tibble::tibble(
    tier = s$tier,
    cell = s$cell,
    path = s$path,
    rows = s$rows,
    tiles = s$tiles,
    bytes = s$bytes,
    footer_bytes = s$footer_bytes,
    xmin = lon$xmin,
    ymin = s$south,
    xmax = lon$xmax,
    ymax = s$north
  )
  t <- r$tiers
  tier <- seq_along(t$lod_min) - 1L
  per_tier <- function(v) vapply(tier, function(k) sum(v[s$tier == k]), double(1))
  tiers <- tibble::tibble(
    tier = tier,
    lod_min = t$lod_min,
    lod_max = t$lod_max,
    shard_resolution = t$shard_resolution,
    tile_size = t$tile_size,
    shards = as.integer(per_tier(rep(1, length(s$tier)))),
    rows = per_tier(s$rows),
    bytes = per_tier(s$bytes)
  )
  bbox_lon <- stack_lon(r$bbox[1], r$bbox[3])
  meta <- r$metadata$value
  names(meta) <- r$metadata$key
  structure(
    list(
      path = dir,
      format_version = r$format_version,
      generator = r$generator,
      a5_crate = r$a5_crate,
      a5_index = r$a5_index,
      data_resolution = r$data_resolution,
      bbox = c(xmin = bbox_lon$xmin, ymin = r$bbox[2], xmax = bbox_lon$xmax, ymax = r$bbox[4]),
      columns = tibble::new_tibble(r$columns, nrow = length(r$columns$name)),
      tiers = tiers,
      shards = shards,
      metadata = meta
    ),
    class = "a5pyramid_set_info"
  )
}

#' @export
print.a5pyramid_set_info <- function(x, ...) {
  cols <- if (nrow(x$columns) > 0L) {
    paste0(x$columns$name, " <", x$columns$type, ", ", x$columns$reducer, ">", collapse = ", ")
  } else {
    "none"
  }
  tier_lines <- vapply(seq_len(nrow(x$tiers)), function(i) {
    t <- x$tiers[i, ]
    footers <- x$shards$footer_bytes[x$shards$tier == t$tier]
    paste0(
      "LODs ", t$lod_min, "-", t$lod_max, ", shards at resolution ", t$shard_resolution, ": ",
      format(t$shards, big.mark = ","), " shard", if (t$shards == 1L) "" else "s", ", ",
      format(t$rows, big.mark = ","), " rows, ", format_bytes(t$bytes),
      " (footers up to ", format_bytes(max(c(0, footers))), ")"
    )
  }, character(1))
  names(tier_lines) <- paste("Tier", x$tiers$tier)
  b <- round(x$bbox, 4)
  cli::cli_rule(left = "a5pyramid set {.path {basename(x$path)}}")
  cli::cli_dl(c(
    "Size" = paste0(format_bytes(sum(x$shards$bytes)), " in ", format(nrow(x$shards), big.mark = ","), " shards"),
    "A5 index" = x$a5_index,
    "Leaves" = paste0("resolution ", x$data_resolution),
    tier_lines,
    "Columns" = cols,
    "Bbox" = paste(b, collapse = ", "),
    "Written by" = paste0(x$generator, ", a5 crate ", x$a5_crate)
  ))
  invisible(x)
}

#' Validate `tiers` for a5_write_pyramid_set(): NULL, or a data frame with
#' whole-number lod_min, lod_max and shard_resolution, and tile_size
#' (default 16384). The engine checks how the tiers fit the levels.
#' @noRd
check_tiers <- function(tiers, call = rlang::caller_env()) {
  if (is.null(tiers)) {
    return(NULL)
  }
  if (!is.data.frame(tiers) || nrow(tiers) == 0L) {
    a5pyramid_abort("input", "{.arg tiers} must be a data frame with one row per tier.", call = call)
  }
  need <- c("lod_min", "lod_max", "shard_resolution")
  missing <- setdiff(need, names(tiers))
  if (length(missing) > 0L) {
    a5pyramid_abort("input", "{.arg tiers} must have columns {.field {need}}; missing {.field {missing}}.", call = call)
  }
  if (is.null(tiers$tile_size)) {
    tiers$tile_size <- 16384L
  }
  out <- list()
  for (nm in c(need, "tile_size")) {
    v <- tiers[[nm]]
    ok <- is.numeric(v) && !anyNA(v) && all(v == trunc(v)) && all(v >= 0) && all(v <= .Machine$integer.max)
    if (!ok) {
      a5pyramid_abort("input", "{.arg tiers}{.field ${nm}} must hold whole numbers of at least 0.", call = call)
    }
    out[[nm]] <- as.integer(v)
  }
  out
}

#' Every LOD the tiers hold below the leaves' resolution
#' @noRd
tier_lods <- function(tiers, resolution) {
  ranges <- Map(function(lo, hi) if (lo <= hi) lo:hi else integer(), tiers$lod_min, tiers$lod_max)
  all <- sort(unique(unlist(ranges)))
  as.integer(all[all < resolution])
}
