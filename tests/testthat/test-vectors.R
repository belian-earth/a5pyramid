# Vector columns: a fixed number of 8-bit values per cell (an embedding).

vector_leaves <- function(seed = 1L, dim = 6L) {
  x <- make_leaves(seed)
  n <- nrow(x)
  x$emb <- matrix(sample(-128:127, n * dim, replace = TRUE), n, dim)
  x
}

test_that("a vector chooses as its elements would as rep_child columns of one scale", {
  x <- vector_leaves()
  sd <- 70
  v <- a5_pyramid(x, emb, lods = c(10, 12), rep_sd = c(emb = sd))
  expect_true(is.matrix(v$emb))
  expect_identical(dim(v$emb), c(nrow(v), 6L))
  y <- x
  for (e in 1:6) y[[paste0("e", e)]] <- x$emb[, e]
  cols <- paste0("e", 1:6)
  s <- a5_pyramid(y, tidyselect::all_of(cols), aggregate = "rep_child", lods = c(10, 12),
                  types = setNames(rep("int8", 6), cols), rep_sd = setNames(rep(sd, 6), cols))
  v <- v[order(v$lod, hex(v$cell)), ]
  s <- s[order(s$lod, hex(s$cell)), ]
  expect_identical(hex(v$cell), hex(s$cell))
  expect_identical(unname(v$emb), unname(as.matrix(s[cols])))
})

test_that("every coarse cell holds one leaf's whole vector, with the rep_child columns", {
  x <- vector_leaves()
  x$red <- sample(0:255, nrow(x), replace = TRUE)
  p <- a5_pyramid(x, red, emb, aggregate = "rep_child", types = c(red = "uint8"), lods = 11L)
  leaf_rows <- paste(x$red, apply(x$emb, 1, paste, collapse = ","))
  rows <- paste(p$red, apply(p$emb, 1, paste, collapse = ","))
  expect_true(all(rows %in% leaf_rows))
  # the leaves keep their own vectors
  leaves <- p[p$lod == 15L, ]
  m <- match(hex(leaves$cell), hex(x$cell))
  expect_identical(unname(leaves$emb), unname(x$emb[m, ]))
})

test_that("vectors round-trip through a version 3 file and a set", {
  x <- vector_leaves(dim = 8L)
  path <- tmp_parquet()
  u <- x
  u$emb <- x$emb + 128L
  a5_write_pyramid(u, path, value, emb, types = c(emb = "uint8"), tile_size = 300)
  info <- a5_pyramid_info(path)
  expect_identical(info$format_version, 3L)
  expect_identical(info$columns$dim, c(NA, 8L))
  expect_identical(info$columns$reducer, c("mean", "rep_child"))
  back <- a5_read_pyramid(path, lod = 15L)
  m <- match(hex(back$cell), hex(x$cell))
  expect_identical(unname(back$emb), unname(x$emb[m, ] + 128L))

  dest <- withr::local_tempdir()
  unlink(dest, recursive = TRUE)
  tiers <- data.frame(lod_min = c(2, 13), lod_max = c(12, 15), shard_resolution = c(0, 9), tile_size = 500)
  a5_write_pyramid_set(x, dest, emb, tiers = tiers)
  set <- a5_pyramid_set_info(dest)
  expect_identical(set$format_version, 3L)
  expect_identical(set$columns$dim, 8L)
  rows <- do.call(rbind, lapply(file.path(dest, set$shards$path), function(f) {
    r <- a5_read_pyramid(f)
    data.frame(cell = hex(r$cell), lod = r$lod, emb = apply(r$emb, 1, paste, collapse = ","))
  }))
  want <- a5_pyramid(x, emb, lods = 2:14)
  want <- data.frame(cell = hex(want$cell), lod = want$lod, emb = apply(want$emb, 1, paste, collapse = ","))
  key <- function(d) d[order(d$lod, d$cell), ]
  expect_equal(key(rows), key(want), ignore_attr = TRUE)
})

test_that("Arrow fixed-size lists and binary are vector columns", {
  skip_if_not_installed("arrow")
  x <- vector_leaves(dim = 4L)
  want <- a5_pyramid(x, emb, lods = 12L)
  want <- want[order(want$lod, hex(want$cell)), ]
  rows <- lapply(seq_len(nrow(x)), function(i) x$emb[i, ])
  tbl <- arrow::arrow_table(
    cell = a5R::a5_cell_to_arrow(x$cell),
    emb = arrow::Array$create(rows, type = arrow::fixed_size_list_of(arrow::int8(), 4L))
  )
  got <- a5_pyramid(tbl, lods = 12L)
  got <- got[order(got$lod, hex(got$cell)), ]
  expect_identical(unname(got$emb), unname(want$emb))
  info_path <- tmp_parquet()
  a5_write_pyramid(tbl, info_path, lods = 12L)
  expect_identical(a5_pyramid_info(info_path)$columns$type, "int8")

  # fixed-size binary reads as uint8: as the same bytes in a uint8 matrix
  u <- x
  u$emb <- (x$emb + 256L) %% 256L
  want_u <- a5_pyramid(u, emb, types = c(emb = "uint8"), lods = 12L)
  want_u <- want_u[order(want_u$lod, hex(want_u$cell)), ]
  raw_rows <- lapply(seq_len(nrow(u)), function(i) as.raw(u$emb[i, ]))
  bin <- arrow::arrow_table(
    cell = a5R::a5_cell_to_arrow(x$cell),
    emb = arrow::Array$create(raw_rows, type = arrow::fixed_size_binary(4L))
  )
  got <- a5_pyramid(bin, lods = 12L)
  got <- got[order(got$lod, hex(got$cell)), ]
  expect_identical(unname(got$emb), unname(want_u$emb))
})

test_that("bad vector columns are refused", {
  x <- vector_leaves()
  err <- function(...) expect_error(a5_pyramid(x, emb, ...), class = "a5pyramid_error_input")
  err(reducers = c(emb = "mean"))
  err(types = c(emb = "int16"))
  err(rep_sd = c(emb = 0))
  y <- x
  y$emb[3, 2] <- 300L
  expect_error(a5_pyramid(y, emb), "outside int8", class = "a5pyramid_error_input")
  y$emb[3, 2] <- NA
  expect_error(a5_pyramid(y, emb), "NA", class = "a5pyramid_error_input")
})

test_that("non-nullable list children, int8 binary and null vectors on dropped rows", {
  skip_if_not_installed("arrow")
  x <- vector_leaves(dim = 4L)
  want <- a5_pyramid(x, emb, lods = 12L)
  want <- want[order(want$lod, hex(want$cell)), ]
  sorted <- function(p) p[order(p$lod, hex(p$cell)), ]
  rows <- lapply(seq_len(nrow(x)), function(i) x$emb[i, ])

  # a list whose child is not nullable, as pyarrow can write
  child <- arrow::field("item", arrow::int8(), nullable = FALSE)
  strict <- arrow::arrow_table(
    cell = a5R::a5_cell_to_arrow(x$cell),
    emb = arrow::Array$create(rows, type = arrow::fixed_size_list_of(child, 4L))
  )
  expect_identical(unname(sorted(a5_pyramid(strict, lods = 12L))$emb), unname(want$emb))

  # fixed-size binary holding int8 bytes, as arrow reads a version-3 file
  bin <- arrow::arrow_table(
    cell = a5R::a5_cell_to_arrow(x$cell),
    emb = arrow::Array$create(lapply(rows, function(r) as.raw(bitwAnd(r, 255L))), type = arrow::fixed_size_binary(4L))
  )
  expect_identical(unname(sorted(a5_pyramid(bin, types = c(emb = "int8"), lods = 12L))$emb), unname(want$emb))
  expect_error(a5_pyramid(strict, types = c(emb = "uint8")), "comes from the data", class = "a5pyramid_error_input")

  # a row without a cell is dropped, missing vector and all
  cells <- a5R::a5_cell_to_arrow(c(x$cell, a5R::a5_cell(NA)))
  holey <- arrow::arrow_table(
    cell = cells,
    emb = arrow::Array$create(c(rows, list(NULL)), type = arrow::fixed_size_list_of(arrow::int8(), 4L))
  )
  expect_identical(unname(sorted(a5_pyramid(holey, lods = 12L))$emb), unname(want$emb))
})
