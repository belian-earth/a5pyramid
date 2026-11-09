skip_if_not_installed("arrow")

types <- c(value = "float32", count = "uint8")

test_that("each record batch is the row group of the same pyramid's file", {
  x <- make_leaves()
  x$value[4] <- NA
  meta <- c(app = '{"k":1}')
  tbl <- a5_pyramid_arrow(x, value, count, types = types, tile_size = 300, metadata = meta)
  path <- tmp_parquet()
  a5_write_pyramid(x, path, value, count, types = types, tile_size = 300, metadata = meta)

  expect_s3_class(tbl, "Table")
  batches <- arrow::as_record_batch_reader(tbl)$batches()
  info <- a5_pyramid_info(path)
  expect_length(batches, nrow(info$tiles))
  expect_identical(vapply(batches, function(b) b$num_rows, numeric(1)), info$tiles$rows)

  reader <- arrow::ParquetFileReader$create(path)
  cols <- c("lod", "ncell", "value", "count")
  for (i in seq_along(batches)) {
    rg <- reader$ReadRowGroup(i - 1L)
    b <- batches[[i]]
    expect_identical(
      hex(a5R::a5_cell_from_arrow(b$cell)),
      hex(a5R::a5_cell_from_arrow(rg$cell))
    )
    for (col in cols) expect_identical(as.vector(b[[col]]), as.vector(rg[[col]]))
  }

  # the same metadata as the file: index, A5 index version, caller's entries
  file_kv <- reader$GetSchema()$metadata
  expect_identical(tbl$metadata$a5pyramid, file_kv$a5pyramid)
  expect_identical(tbl$metadata$a5_index, "1.0")
  expect_identical(tbl$metadata$app, '{"k":1}')
})

test_that("the table holds the pyramid with the file's types", {
  x <- make_leaves()
  tbl <- a5_pyramid_arrow(x, value, count, class, types = types, reducers = c(class = "mode"))
  type_of <- function(nm) tbl$schema[[nm]]$type$ToString()
  expect_identical(
    vapply(names(tbl), type_of, character(1)),
    c(cell = "uint64", lod = "uint8", ncell = "uint32", value = "float", count = "uint8", class = "int32")
  )
  # a5R reads it back as cells and values, as built
  back <- a5R::a5_from_arrow_table(tbl)
  built <- a5_pyramid(x, value, count, class, types = types, reducers = c(class = "mode"))
  expect_identical(hex(back$cell), hex(built$cell))
  expect_equal(back$value, built$value, tolerance = 1e-6)
  expect_identical(as.integer(back$count), built$count)
  expect_identical(as.integer(back$lod), built$lod)
})

test_that("a pyramid of bare cells has no value columns", {
  tbl <- a5_pyramid_arrow(cap_cells(res = 12L), lods = c(6, 9))
  expect_named(tbl, c("cell", "lod", "ncell"))
  expect_identical(sort(unique(as.vector(tbl$lod))), c(6L, 9L, 12L))
})

test_that("bad input is refused before anything is built", {
  x <- make_leaves()
  expect_error(a5_pyramid_arrow(x, tile_size = 0), class = "a5pyramid_error_input")
  expect_error(a5_pyramid_arrow(x, metadata = c(a5_index = "2")), class = "a5pyramid_error_input")
  expect_error(a5_pyramid_arrow(rbind(x, x[1, ])), "more than once", class = "a5pyramid_error_input")
})

test_that("a stream that is never imported is released", {
  x <- make_leaves()
  input <- pyramid_input(x, list(), NULL, "mean", NULL, NULL, NULL)
  stream <- a5pyramid_arrow_rs(input$leaves, input$lods, 4096L, character(), character(), "test", 2L)
  expect_match(stream$address, "^[0-9]+$")
  rm(stream)
  expect_no_error(gc())
  # the leaves went into that build
  expect_error(a5pyramid_build_rs(input$leaves, input$lods, 1L), "already built")
})

# Leaves as Arrow data: cells as uint64, the values in their own types.
as_arrow_leaves <- function(x) {
  arrow::arrow_table(
    cell = a5R::a5_cell_to_arrow(x$cell),
    value = arrow::Array$create(x$value, type = arrow::float64()),
    count = arrow::Array$create(x$count, type = arrow::uint8()),
    class = arrow::Array$create(x$class, type = arrow::int8())
  )
}

test_that("Arrow data builds the pyramid of the same data frame", {
  x <- make_leaves()
  x$value[3] <- NA
  tbl <- as_arrow_leaves(x)
  types <- c(value = "float64", count = "uint8", class = "int8")
  want <- a5_pyramid(x, value, count, class, types = types, reducers = c(class = "mode"))
  # a dataset of three files (write_dataset() would need dplyr)
  dir <- withr::local_tempdir()
  cut <- floor(nrow(tbl) * c(0, 1 / 3, 2 / 3, 1))
  for (k in 1:3) {
    arrow::write_parquet(tbl$Slice(cut[k], cut[k + 1] - cut[k]), file.path(dir, sprintf("part-%d.parquet", k)))
  }
  sources <- list(
    table = tbl,
    batch = arrow::as_record_batch(tbl),
    reader = arrow::as_record_batch_reader(tbl),
    dataset = arrow::open_dataset(dir)
  )
  for (nm in names(sources)) {
    # each column takes its Arrow type unless `types` says otherwise
    got <- a5_pyramid(sources[[nm]], reducers = c(class = "mode"))
    got <- got[order(got$lod, hex(got$cell)), ]
    ref <- want[order(want$lod, hex(want$cell)), ]
    expect_identical(hex(got$cell), hex(ref$cell), info = nm)
    for (col in c("lod", "ncell", "value", "count", "class")) {
      expect_identical(got[[col]], ref[[col]], info = paste(nm, col))
    }
  }
})

test_that("Arrow data: null cells drop their rows, weights are read by name", {
  x <- make_leaves()
  x$w <- as.double(x$count)
  tbl <- arrow::arrow_table(
    cell = a5R::a5_cell_to_arrow(c(x$cell[1:5], a5R::a5_cell(NA), x$cell[-(1:5)])),
    value = c(x$value[1:5], 99, x$value[-(1:5)]),
    w = c(x$w[1:5], 1, x$w[-(1:5)])
  )
  got <- a5_pyramid(tbl, value, weights = "w", lods = 12L)
  want <- a5_pyramid(x, value, weights = "w", lods = 12L)
  expect_identical(nrow(got), nrow(want))
  expect_equal(sum(got$ncell[got$lod == 12L]), nrow(x))
  expect_equal(level(got, 12L)$value, level(want, 12L)$value)
})

test_that("Arrow data without one uint64 cell column is refused", {
  x <- make_leaves()
  tbl <- as_arrow_leaves(x)
  expect_error(a5_pyramid(tbl$SelectColumns(1:3)), "one .*uint64", class = "a5pyramid_error_input")
  two <- arrow::arrow_table(a = tbl$cell, b = tbl$cell, v = tbl$value)
  expect_error(a5_pyramid(two), "It has 2", class = "a5pyramid_error_input")
  old <- tbl$cast(tbl$schema$WithMetadata(list(a5_index = "0.6")))
  expect_error(a5_pyramid(old), "A5 index 0.6", class = "a5pyramid_error_input")
  big <- arrow::arrow_table(cell = tbl$cell, n = arrow::Array$create(seq_len(nrow(x)), type = arrow::int64()))
  expect_identical(setdiff(names(a5_pyramid(big, lods = 12L)), c("cell", "lod", "ncell")), character())
  expect_error(a5_pyramid(big, n), "must be numeric", class = "a5pyramid_error_input")
})
