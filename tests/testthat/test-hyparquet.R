# The format's purpose: a browser reads it with hyparquet. These tests run
# the hyparquet that a5view vendors under Node, through a byte-range buffer.

hyparquet_path <- function() {
  path <- system.file("htmlwidgets/lib/hyparquet/hyparquet.esm.js", package = "a5view")
  if (!nzchar(path)) skip("a5view (which vendors hyparquet) is not installed")
  if (!nzchar(Sys.which("node"))) skip("Node.js is not installed")
  path
}

read_hyparquet <- function(path, rg, script = "read-hyparquet.mjs") {
  script <- test_path("js", script)
  # an .mjs copy loads as an ES module on every Node version; Node 18 does
  # not detect module syntax in a .js file
  lib <- withr::local_tempfile(fileext = ".mjs")
  file.copy(hyparquet_path(), lib)
  out <- system2("node", c(shQuote(script), shQuote(lib), shQuote(path), rg),
                 stdout = TRUE, stderr = TRUE)
  status <- attr(out, "status")
  if (!is.null(status) && status != 0L) stop(paste(out, collapse = "\n"))
  jsonlite::fromJSON(paste(out, collapse = ""), simplifyVector = TRUE)
}

test_that("hyparquet reads a row group as a5pyramid wrote it", {
  hyparquet_path()
  skip_if_not_installed("jsonlite")
  x <- make_leaves()
  x$value[5] <- NA
  path <- tmp_parquet()
  a5_write_pyramid(x, path, value, count, types = c(value = "float32", count = "uint8"), tile_size = 250)
  info <- a5_pyramid_info(path)
  rows <- a5_read_pyramid(path)
  starts <- cumsum(c(0, info$tiles$rows))

  leaf_rgs <- info$tiles$tile[info$tiles$lod == 15L]
  for (rg in c(0L, leaf_rgs[2])) {
    js <- read_hyparquet(path, rg)
    want <- rows[(starts[rg + 1] + 1):starts[rg + 2], ]
    expect_identical(js$a5_index, "1.0")
    expect_identical(js$lods, info$lods)
    expect_identical(js$n_row_groups, nrow(info$tiles))
    expect_identical(js$cell_type, "bigint")
    expect_identical(js$cells, hex(want$cell))
    expect_identical(js$lod, want$lod)
    expect_identical(js$ncell, want$ncell)
    expect_equal(js$columns$value, want$value)
    expect_identical(as.integer(js$columns$count), want$count)
    # only the footer and this row group's column chunks were fetched
    expect_lte(js$fetched_bytes, js$row_group_bytes)
    expect_lt(js$fetched_bytes, info$bytes / 4)
  }
})

test_that("hyparquet reads a shard of a set, footer in one request", {
  hyparquet_path()
  skip_if_not_installed("jsonlite")
  x <- make_leaves()
  dest <- withr::local_tempfile()
  tiers <- data.frame(lod_min = c(2L, 13L), lod_max = c(12L, 15L),
                      shard_resolution = c(0L, 10L), tile_size = c(100L, 300L))
  a5_write_pyramid_set(x, dest, value, types = c(value = "float32"), tiers = tiers)
  set <- a5_pyramid_set_info(dest)
  sh <- set$shards[set$shards$tier == 1L, ][1, ]
  path <- file.path(dest, sh$path)
  info <- a5_pyramid_info(path)
  rows <- a5_read_pyramid(path)
  js <- read_hyparquet(path, 0L)
  expect_identical(js$format_version, 2L)
  expect_identical(js$shard$cell, hex(sh$cell))
  expect_identical(js$shard$tier, 1L)
  expect_identical(js$lods, 13:15)
  expect_identical(js$footer_fetches, 1L)
  expect_lt(sh$footer_bytes, 512 * 1024)
  first <- seq_len(info$tiles$rows[1])
  expect_identical(js$cells, hex(rows$cell[first]))
  expect_equal(js$columns$value, rows$value[first])
})

test_that("hyparquet reads a vector column as one byte array a row", {
  hyparquet_path()
  skip_if_not_installed("jsonlite")
  x <- make_leaves()
  x$emb <- matrix(sample(-128:127, nrow(x) * 16, replace = TRUE), nrow(x), 16)
  path <- tmp_parquet()
  a5_write_pyramid(x, path, value, emb, tile_size = 250)
  info <- a5_pyramid_info(path)
  rows <- a5_read_pyramid(path)
  starts <- cumsum(c(0, info$tiles$rows))
  rg <- info$tiles$tile[info$tiles$lod == 15L][1]
  js <- read_hyparquet(path, rg)
  want <- rows[(starts[rg + 1] + 1):starts[rg + 2], ]
  expect_identical(js$format_version, 3L)
  expect_identical(js$vector_class$emb, "Uint8Array")
  expect_identical(js$cells, hex(want$cell))
  expect_identical(unname(js$vectors$emb), unname(want$emb))
})

test_that("hyparquet reads a tile of a row group of several tiles by its pages", {
  hyparquet_path()
  skip_if_not_installed("jsonlite")
  x <- make_leaves()
  x$value[5] <- NA
  path <- tmp_parquet()
  a5_write_pyramid(x, path, value, count, types = c(value = "float32", count = "uint8"),
                   tile_size = 100, row_group_size = 800)
  info <- a5_pyramid_info(path)
  rows <- a5_read_pyramid(path)
  starts <- cumsum(c(0, info$tiles$rows))
  # the third tile of a leaf row group of eight
  leaf <- info$tiles[info$tiles$lod == 15L, ]
  g <- as.integer(names(which(table(leaf$row_group) == 8L))[1])
  tile <- leaf$tile[leaf$row_group == g][3]
  js <- read_hyparquet(path, tile, "read-hyparquet-tile.mjs")
  want <- rows[(starts[tile + 1] + 1):starts[tile + 2], ]
  expect_identical(js$format_version, 4L)
  expect_identical(js$n_tiles, nrow(info$tiles))
  expect_identical(js$n_row_groups, info$row_groups)
  expect_identical(c(js$row_group, js$page), c(g, 2L))
  expect_identical(js$cells, hex(want$cell))
  expect_identical(js$lod, want$lod)
  expect_identical(js$ncell, want$ncell)
  expect_equal(js$columns$value, want$value)
  expect_identical(as.integer(js$columns$count), want$count)
  # its pages, the offset index and the lod dictionary, not the row group
  expect_lt(js$fetched_bytes, js$row_group_bytes / 2)
})
