#' Partial rows of a pyramid's mean vectors
#'
#' Reduces leaves to exact partial sums at coarse levels, so that pyramids
#' of separate parts of a region, each built alone (a store's partitions),
#' can have coarse levels that span the parts. [a5_pyramid_merge_partials()]
#' adds the partial rows of every part and encodes the means: the rows that
#' [a5_pyramid()] gives for all the leaves at once.
#'
#' A row holds a cell of a LOD that has a leaf (the cell containing the
#' leaf's centre, [a5R::a5_cell_to_supercell()]) and the sums over its
#' leaves: their number, their weights, and for each vector the weighted sum
#' of every element's value. With a codec the values are lifted as the
#' builder lifts them for a mean (for `"aef"`, `code * |code|`), so with
#' integer weights the sums are integers, exact in doubles below 2^53, and
#' the merged means do not depend on how the leaves were split.
#'
#' @inheritParams a5_pyramid
#' @param ... The vector columns to sum (tidyselect, as in [a5_pyramid()]).
#'   Each is reduced by its mean; other columns do not merge and are
#'   refused.
#' @param lods The coarse LODs, A5 resolutions below the leaves'.
#' @param key_resolution The resolution of each row's key, the supercell of
#'   its cell there: the shard its merged row belongs to. One for every
#'   LOD, or one for all, each no finer than its LOD.
#' @param weights The leaf weights column, every weight above zero (a mean
#'   over leaves that all weigh nothing has no partial form), or `NULL` for
#'   a weight of 1 each.
#' @param weights_type The type the merged rows store their summed weights
#'   in, as `weights_type` in [a5_pyramid()]; ignored without `weights`.
#' @returns An Arrow `Table` in ascending LOD and cell order, with the
#'   columns `lod` (`uint8`), `key` and `cell` (`uint64`), `ncell`
#'   (`uint64`), the summed weights named as `weights` (`float64`) with
#'   weights, and each vector's sums named as the vector (a fixed-size list
#'   of `float64`). Its schema metadata (`"a5pyramid_partials"`) records the
#'   vectors' types, sizes and codecs and the weights' name and type, which
#'   [a5_pyramid_merge_partials()] needs: [arrow::write_parquet()] keeps it.
#' @seealso [a5_pyramid_merge_partials()] to merge them,
#'   [a5_write_pyramid_level()] to write the merged rows.
#' @export
#' @examplesIf requireNamespace("arrow", quietly = TRUE)
#' library(a5R)
#' centre <- a5_lonlat_to_cell(-3.19, 55.95, resolution = 9)
#' cells <- a5_uncompact(a5_spherical_cap(centre, 4000), resolution = 14)
#' emb <- matrix(sample(-128:127, length(cells) * 4, replace = TRUE), ncol = 4)
#' x <- tibble::tibble(cell = cells, embedding = emb, n = 1 + seq_along(cells) %% 5)
#'
#' # two parts, built one at a time
#' west <- a5_cell_to_supercell(cells, 10) < a5_cell_to_supercell(centre, 10)
#' parts <- lapply(split(seq_along(cells), west), function(i) {
#'   a5_pyramid_partials(x[i, ], embedding, lods = 10:12, key_resolution = 8,
#'                       weights = "n", codecs = c(embedding = "aef"))
#' })
#' merged <- a5_pyramid_merge_partials(do.call(arrow::concat_tables, parts))
#' merged
a5_pyramid_partials <- function(x,
                                ...,
                                lods,
                                key_resolution,
                                weights = NULL,
                                weights_type = "uint32",
                                types = NULL,
                                codecs = NULL,
                                threads = NULL) {
  rlang::check_installed("arrow", reason = "to return Arrow tables")
  if (missing(lods) || is.null(lods) || length(lods) == 0L) {
    a5pyramid_abort("input", "{.arg lods} must name the coarse LODs to take partials at.")
  }
  if (missing(key_resolution)) {
    a5pyramid_abort("input", "{.arg key_resolution} must give the resolution of each row's key.")
  }
  key_resolution <- check_resolutions(key_resolution, "key_resolution")
  threads <- resolve_threads(threads)
  if (is.null(weights)) weights_type <- NULL
  input <- pyramid_input(x, rlang::enquos(...), lods, "mean", NULL, weights, types, NULL, codecs,
                         weights_type = weights_type, vector_reducer = "mean")
  stream <- rs_call(a5pyramid_partials_rs(
    leaves = input$leaves,
    lods = input$lods,
    key_resolutions = key_resolution,
    threads = threads
  ))
  arrow::RecordBatchReader$import_from_c(stream$address)$read_table()
}

#' Merge partial rows into a pyramid's coarse levels
#'
#' Adds the partial rows ([a5_pyramid_partials()]) of every part of a
#' region, cell by cell, and encodes each mean as [a5_pyramid()] does: the
#' code of the mean of the lifted values. The rows of one LOD and key are
#' one shard of the coarse levels, ready for [a5_write_pyramid_level()]
#' (with `ncell = "ncell"` and the weights).
#'
#' @param x Partial rows: an Arrow `Table`, `RecordBatchReader` or `Dataset`
#'   (such as [arrow::open_dataset()] over the parts' Parquet files), with
#'   the schema metadata [a5_pyramid_partials()] writes. Batches are read one
#'   at a time, and only the rows of `keys` are kept.
#' @param keys Optional [a5R::a5_cell] vector (or hex strings): merge only
#'   the rows of these keys, such as one shard's.
#' @param threads Number of threads, or `NULL` (default) for
#'   [a5pyramid_threads()].
#' @returns An Arrow `Table` in ascending LOD, key and cell order: `lod`
#'   (`uint8`), `key` and `cell` (`uint64`), `ncell` (`uint32`), the summed
#'   weights in their stored type (named as the weights), and each vector's
#'   codes, a fixed-size list of its element type.
#' @seealso [a5_pyramid_partials()].
#' @export
a5_pyramid_merge_partials <- function(x, keys = NULL, threads = NULL) {
  rlang::check_installed("arrow", reason = "to read and return Arrow tables")
  threads <- resolve_threads(threads)
  if (!is.null(keys)) {
    if (is.character(keys)) keys <- a5R::a5_cell(keys)
    if (!a5R::is_a5_cell(keys) || anyNA(keys)) {
      a5pyramid_abort("input", "{.arg keys} must be A5 cells (an {.cls a5_cell} vector or hex strings) without NA.")
    }
  }
  reader <- tryCatch(arrow::as_record_batch_reader(x), error = function(e) {
    a5pyramid_abort("input", "{.arg x} must be Arrow data: a Table, RecordBatchReader or Dataset of partial rows.")
  })
  slot <- a5pyramid_stream_slot_rs()
  reader$export_to_c(slot$address)
  stream <- rs_call(a5pyramid_merge_partials_rs(slot$holder, keys, threads))
  arrow::RecordBatchReader$import_from_c(stream$address)$read_table()
}

#' Whole, non-negative resolutions
#' @noRd
check_resolutions <- function(x, arg, call = rlang::caller_env()) {
  ok <- is.numeric(x) && length(x) > 0L && !anyNA(x) && all(x == trunc(x)) && all(x >= 0 & x <= 30)
  if (!ok) {
    a5pyramid_abort("input", "{.arg {arg}} must be whole A5 resolutions from 0 to 30.", call = call)
  }
  as.integer(x)
}
