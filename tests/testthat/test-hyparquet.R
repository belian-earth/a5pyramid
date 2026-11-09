# The format's purpose: a browser reads it with hyparquet. These tests run
# the hyparquet that a5view vendors under Node, through a byte-range buffer.

hyparquet_path <- function() {
  path <- system.file("htmlwidgets/lib/hyparquet/hyparquet.esm.js", package = "a5view")
  if (!nzchar(path)) skip("a5view (which vendors hyparquet) is not installed")
  if (!nzchar(Sys.which("node"))) skip("Node.js is not installed")
  path
}

read_hyparquet <- function(path, rg) {
  script <- test_path("js", "read-hyparquet.mjs")
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
