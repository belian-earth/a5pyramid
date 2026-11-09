#' Inspect a pyramid file
#'
#' Reads the footer of a file written by [a5_write_pyramid()], its index and
#' Parquet layout, without reading any rows. This is what a browser fetches
#' first, so `footer_bytes` is the cost of opening the file there.
#'
#' Longitudes follow the a5R stack's convention: a box across the
#' antimeridian has `xmax` above 180, as [a5R::a5_bbox()] reports. The file
#' itself stores such a box with `west > east`, as GeoJSON does.
#'
#' @param src Path of a pyramid Parquet file.
#' @returns A list of class `a5pyramid_info`:
#'   - `path`, `bytes` (file size), `footer_bytes` (Parquet footer, index
#'     included).
#'   - `format_version`, `generator`, `a5_crate` (the a5 crate that built
#'     the file) and `a5_index` (the A5 index version of its ids).
#'   - `data_resolution`: the leaves' resolution. `lods`: every level's
#'     LOD, ascending, the leaves last.
#'   - `tile_size`: the most rows in a tile.
#'   - `bbox`: `c(xmin, ymin, xmax, ymax)` of the leaves.
#'   - `columns`: a tibble of the value columns' `name`, `type` and
#'     `reducer`.
#'   - `levels`: a tibble of each level's `lod`, `rows` and number of
#'     `tiles`.
#'   - `tiles`: a tibble with one row per tile, which is one row group of
#'     the file: `tile` (its 0-based index, the row group's index as Parquet
#'     readers count), `lod`, `rows`, `bytes` (compressed size, what a
#'     reader fetches) and `xmin`, `ymin`, `xmax`, `ymax`.
#'   - `metadata`: the file's other key-value metadata, a named character
#'     vector.
#'   - `shard`: for a shard of a pyramid set (format version 2, written by
#'     [a5_write_pyramid_set()]), a list of its `tier`, shard `cell` (hex)
#'     and its shard `resolution`; `NULL` for a single file.
#' @seealso [a5_read_pyramid()] to read rows.
#' @export
#' @examples
#' library(a5R)
#' centre <- a5_lonlat_to_cell(-3.19, 55.95, resolution = 10)
#' cells <- a5_uncompact(a5_spherical_cap(centre, 3000), resolution = 15)
#' path <- tempfile(fileext = ".parquet")
#' a5_write_pyramid(data.frame(cell = cells, value = runif(length(cells))), path)
#'
#' info <- a5_pyramid_info(path)
#' info
#' info$levels
a5_pyramid_info <- function(src) {
  src <- check_src(src)
  r <- rs_call(a5pyramid_info_rs(src))
  tiles <- r$tiles
  tile_lon <- stack_lon(tiles$west, tiles$east)
  bbox_lon <- stack_lon(r$bbox[1], r$bbox[3])
  meta <- r$metadata$value
  names(meta) <- r$metadata$key
  structure(
    list(
      path = src,
      bytes = r$bytes,
      footer_bytes = r$footer_bytes,
      format_version = r$format_version,
      generator = r$generator,
      a5_crate = r$a5_crate,
      a5_index = r$a5_index,
      data_resolution = r$data_resolution,
      lods = r$levels$lod,
      tile_size = r$tile_size,
      bbox = c(xmin = bbox_lon$xmin, ymin = r$bbox[2], xmax = bbox_lon$xmax, ymax = r$bbox[4]),
      columns = tibble::new_tibble(r$columns, nrow = length(r$columns$name)),
      levels = tibble::tibble(
        lod = r$levels$lod,
        rows = r$levels$rows,
        tiles = r$levels$tile_end - r$levels$tile_start
      ),
      tiles = tibble::tibble(
        tile = seq_along(tiles$lod) - 1L,
        lod = tiles$lod,
        rows = tiles$rows,
        bytes = tiles$bytes,
        xmin = tile_lon$xmin,
        ymin = tiles$south,
        xmax = tile_lon$xmax,
        ymax = tiles$north
      ),
      metadata = meta,
      shard = r$shard
    ),
    class = "a5pyramid_info"
  )
}

#' @export
print.a5pyramid_info <- function(x, ...) {
  mb <- function(b) format_bytes(b)
  lods <- if (length(x$lods) > 1L) {
    paste0(min(x$lods), "-", max(x$lods), " (", length(x$lods), " levels)")
  } else {
    as.character(x$lods)
  }
  cols <- if (nrow(x$columns) > 0L) {
    paste0(x$columns$name, " <", x$columns$type, ", ", x$columns$reducer, ">", collapse = ", ")
  } else {
    "none"
  }
  b <- round(x$bbox, 4)
  what <- if (is.null(x$shard)) "a5pyramid file" else "a5pyramid shard"
  cli::cli_rule(left = "{what} {.path {basename(x$path)}}")
  cli::cli_dl(c(
    "Size" = paste0(mb(x$bytes), " (footer ", mb(x$footer_bytes), ")"),
    "A5 index" = x$a5_index,
    if (is.null(x$shard)) {
      c("Leaves" = paste0("resolution ", x$data_resolution, ", ", format(utils::tail(x$levels$rows, 1), big.mark = ","), " cells"))
    } else {
      c(
        "Shard" = paste0(x$shard$cell, " (resolution ", x$shard$resolution, "), tier ", x$shard$tier),
        "Leaves" = paste0("resolution ", x$data_resolution)
      )
    },
    "LODs" = lods,
    "Rows" = format(sum(x$levels$rows), big.mark = ","),
    "Tiles" = paste0(format(nrow(x$tiles), big.mark = ","), " row groups (tile size ", x$tile_size, ")"),
    "Columns" = cols,
    "Bbox" = paste(b, collapse = ", "),
    "Written by" = paste0(x$generator, ", a5 crate ", x$a5_crate)
  ))
  invisible(x)
}

format_bytes <- function(b) {
  units <- c("B", "KB", "MB", "GB")
  k <- max(1L, min(length(units), floor(log(max(b, 1), 1000)) + 1L))
  paste(format(round(b / 1000^(k - 1L), if (k == 1L) 0L else 1L), nsmall = if (k == 1L) 0L else 1L), units[k])
}

#' Read levels of a pyramid file
#'
#' Reads the rows of the tiles at `lod` whose bounding boxes meet `bbox`,
#' as a renderer does for a view: the footer first, then only those tiles'
#' row groups. Selection is by tile, so rows of a selected tile that lie
#' outside `bbox` are returned too; filter the cells afterwards for an
#' exact cut.
#'
#' @param src Path of a pyramid Parquet file written by [a5_write_pyramid()].
#' @param lod Integer vector of the LODs to read, each one of the file's
#'   (see [a5_pyramid_info()]), or `NULL` (default) for every level.
#' @param bbox `NULL` (default) for everywhere, or `c(xmin, ymin, xmax,
#'   ymax)` in WGS 84 degrees. For a box across the antimeridian give
#'   `xmin > xmax`, such as `c(179, -1, -179, 1)`.
#' @returns A tibble with the columns of [a5_pyramid()], in file order:
#'   ascending LOD, then cell id.
#' @seealso [a5_pyramid_info()] for the file's levels and tiles.
#' @export
#' @examples
#' library(a5R)
#' centre <- a5_lonlat_to_cell(-3.19, 55.95, resolution = 10)
#' cells <- a5_uncompact(a5_spherical_cap(centre, 3000), resolution = 15)
#' path <- tempfile(fileext = ".parquet")
#' a5_write_pyramid(data.frame(cell = cells, value = runif(length(cells))), path)
#'
#' a5_read_pyramid(path, lod = 12)
#' a5_read_pyramid(path, lod = 15, bbox = c(-3.2, 55.94, -3.18, 55.96))
a5_read_pyramid <- function(src, lod = NULL, bbox = NULL) {
  src <- check_src(src)
  if (!is.null(lod)) {
    ok <- is.numeric(lod) && length(lod) > 0L && !anyNA(lod) && all(lod == trunc(lod))
    if (!ok) {
      a5pyramid_abort("input", "{.arg lod} must be a vector of whole numbers or {.code NULL}.")
    }
    lod <- as.integer(lod)
  }
  bbox <- check_bbox(bbox)
  out <- rs_call(a5pyramid_read_rs(src, lod, bbox))
  check_index(out$a5_index, src)
  pyramid_tibble(out)
}

check_src <- function(src, call = rlang::caller_env()) {
  check_string(src, "src", call = call)
  path <- path.expand(src)
  if (!file.exists(path) || dir.exists(path)) {
    a5pyramid_abort("io", "{.arg src} ({.path {src}}) is not a file.", call = call)
  }
  path
}

#' Stop unless the file's ids are of the A5 index version a5R reads
#' @noRd
check_index <- function(index, src, call = rlang::caller_env()) {
  ours <- a5R::a5_info()$index
  if (!identical(index, ours)) {
    a5pyramid_abort("format", c(
      "{.path {src}} holds A5 {if (is.na(index)) 'cell ids of an unrecorded index version' else paste('index', index, 'cell ids')}, but a5R reads A5 index {ours}.",
      "i" = "Ids of different index versions look alike and draw in the wrong place; rebuild the pyramid."
    ), call = call)
  }
}

#' Longitudes `west`/`east` (west > east across the antimeridian) in the
#' stack's convention, where `xmax` exceeds 180 instead.
#' @noRd
stack_lon <- function(west, east) {
  list(xmin = west, xmax = ifelse(west > east, east + 360, east))
}
