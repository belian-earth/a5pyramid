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
  stream <- a5pyramid_arrow_rs(
    input$cells, input$values, input$names, input$types, input$reducers,
    input$weights, input$lods, 4096L, character(), character(), "test", 2L
  )
  expect_match(stream$address, "^[0-9]+$")
  rm(stream)
  expect_no_error(gc())
})
