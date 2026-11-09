test_that("a written pyramid reads back as built", {
  x <- make_leaves()
  path <- tmp_parquet()
  types <- c(value = "float32", count = "uint8")
  out <- a5_write_pyramid(x, path, types = types, tile_size = 500, metadata = c(app = '{"k":1}'))
  expect_identical(out, path)

  built <- a5_pyramid(x, types = types)
  back <- a5_read_pyramid(path)
  expect_identical(hex(back$cell), hex(built$cell))
  expect_identical(back[-1], built[-1])
})

test_that("the index describes the file", {
  x <- make_leaves()
  path <- tmp_parquet()
  a5_write_pyramid(x, path, value, reducers = c(value = "max"), tile_size = 300,
                   metadata = list(app = "a", note = "b"))
  info <- a5_pyramid_info(path)

  expect_s3_class(info, "a5pyramid_info")
  expect_identical(info$format_version, 1L)
  expect_identical(info$a5_index, a5R::a5_info()$index)
  expect_identical(info$data_resolution, 15L)
  expect_identical(info$lods, 2:15)
  expect_identical(info$columns$name, "value")
  expect_identical(info$columns$type, "float64")
  expect_identical(info$columns$reducer, "max")
  expect_identical(info$metadata, c(app = "a", note = "b"))
  expect_identical(info$levels$rows[info$levels$lod == 15L], as.double(nrow(x)))
  expect_identical(sum(info$levels$tiles), nrow(info$tiles))
  expect_true(all(info$tiles$rows <= 300))
  expect_identical(sum(info$tiles$rows), sum(info$levels$rows))
  expect_identical(info$bytes, file.size(path))
  expect_true(info$footer_bytes > 0 && info$footer_bytes < info$bytes)
  expect_identical(info$tiles$tile, seq_len(nrow(info$tiles)) - 1L)
  # tiles follow the levels, coarsest first
  expect_false(is.unsorted(info$tiles$lod))

  # every leaf lies in the box of its row group, and in the file's box
  leaves <- a5_read_pyramid(path, lod = 15L)
  rg <- info$tiles[info$tiles$lod == 15L, ]
  which_rg <- rep(seq_len(nrow(rg)), rg$rows)
  ll <- unclass(a5R::a5_cell_to_lonlat(leaves$cell))
  expect_true(all(ll$x >= rg$xmin[which_rg] & ll$x <= rg$xmax[which_rg]))
  expect_true(all(ll$y >= rg$ymin[which_rg] & ll$y <= rg$ymax[which_rg]))
  b <- info$bbox
  expect_true(all(ll$x >= b[["xmin"]] & ll$x <= b[["xmax"]] & ll$y >= b[["ymin"]] & ll$y <= b[["ymax"]]))
  expect_match(cli::cli_fmt(print(info))[1], "a5pyramid file")
})

test_that("reading by LOD and bbox returns the tiles in view", {
  x <- make_leaves()
  path <- tmp_parquet()
  a5_write_pyramid(x, path, tile_size = 100)
  view <- c(-3.2, 55.94, -3.18, 55.96)
  part <- a5_read_pyramid(path, lod = 15L, bbox = view)
  all <- a5_read_pyramid(path, lod = 15L)
  expect_lt(nrow(part), nrow(all))
  ll <- unclass(a5R::a5_cell_to_lonlat(all$cell))
  inside <- ll$x >= view[1] & ll$x <= view[3] & ll$y >= view[2] & ll$y <= view[4]
  expect_true(all(hex(all$cell[inside]) %in% hex(part$cell)))

  two <- a5_read_pyramid(path, lod = c(12, 8))
  expect_identical(unique(two$lod), c(8L, 12L))
  far <- a5_read_pyramid(path, lod = 15L, bbox = c(100, 0, 101, 1))
  expect_identical(nrow(far), 0L)
  expect_named(far, names(all))

  expect_error(a5_read_pyramid(path, lod = 1L), "no LOD 1", class = "a5pyramid_error_input")
  expect_error(a5_read_pyramid(path, bbox = c(0, 0, 1)), class = "a5pyramid_error_input")
  expect_error(a5_read_pyramid(path, lod = "a"), class = "a5pyramid_error_input")
})

test_that("boxes across the antimeridian wrap", {
  cells <- c(cap_cells(179.99, -17, 20000, 12L), cap_cells(-179.99, -17, 20000, 12L))
  cells <- unique(cells)
  path <- tmp_parquet()
  a5_write_pyramid(data.frame(cell = cells, v = 1), path, lods = 8L)
  info <- a5_pyramid_info(path)
  # the stack's convention: xmax past 180
  expect_gt(info$bbox[["xmax"]], 180)
  expect_lt(info$bbox[["xmin"]], 180)
  hit <- a5_read_pyramid(path, lod = 12L, bbox = c(179.9, -17.1, -179.9, -16.9))
  expect_gt(nrow(hit), 0L)
  expect_identical(nrow(a5_read_pyramid(path, lod = 12L, bbox = c(0, -17.1, 1, -16.9))), 0L)
})

test_that("every compression reads back", {
  x <- make_leaves()
  for (comp in c("snappy", "zstd", "none")) {
    path <- tmp_parquet()
    a5_write_pyramid(x, path, value, compression = comp)
    expect_equal(a5_read_pyramid(path)$value, a5_pyramid(x, value)$value)
  }
})

test_that("write errors are classed and leave no file", {
  x <- make_leaves()
  dir <- withr::local_tempdir()
  path <- file.path(dir, "p.parquet")
  expect_error(a5_write_pyramid(x, file.path(dir, "no", "p.parquet")), class = "a5pyramid_error_io")
  expect_error(a5_write_pyramid(x, path, metadata = c(a5pyramid = "x")), class = "a5pyramid_error_input")
  expect_error(a5_write_pyramid(x, path, metadata = list(a = 1)), class = "a5pyramid_error_input")
  expect_error(a5_write_pyramid(x, path, metadata = "x"), class = "a5pyramid_error_input")
  expect_error(a5_write_pyramid(x, path, tile_size = 0), class = "a5pyramid_error_input")
  expect_error(a5_write_pyramid(rbind(x, x[1, ]), path), class = "a5pyramid_error_input")
  expect_length(list.files(dir), 0L)

  # an existing file is replaced
  a5_write_pyramid(x[1:10, ], path)
  a5_write_pyramid(x, path)
  expect_identical(a5_pyramid_info(path)$levels$rows[14], as.double(nrow(x)))
  expect_length(list.files(dir), 1L)
})

test_that("files that are not pyramids are refused", {
  expect_error(a5_pyramid_info(tempfile()), class = "a5pyramid_error_io")
  junk <- withr::local_tempfile(fileext = ".parquet")
  writeLines("not parquet", junk)
  expect_error(a5_pyramid_info(junk), class = "a5pyramid_error_format")
  skip_if_not_installed("arrow")
  plain <- withr::local_tempfile(fileext = ".parquet")
  arrow::write_parquet(data.frame(a = 1:3), plain)
  expect_error(a5_read_pyramid(plain), "not an a5pyramid file", class = "a5pyramid_error_format")
})

test_that("Arrow reads the file with its types and index", {
  skip_if_not_installed("arrow")
  x <- make_leaves()
  path <- tmp_parquet()
  a5_write_pyramid(x, path, value, count, types = c(value = "float32", count = "uint8"), tile_size = 400)
  reader <- arrow::ParquetFileReader$create(path)
  schema <- reader$GetSchema()
  expect_identical(schema$cell$type$ToString(), "uint64")
  expect_identical(schema$lod$type$ToString(), "uint8")
  expect_identical(schema$ncell$type$ToString(), "uint32")
  expect_identical(schema$value$type$ToString(), "float")
  expect_identical(schema$count$type$ToString(), "uint8")
  expect_identical(reader$num_row_groups, nrow(a5_pyramid_info(path)$tiles))
  kv <- schema$metadata
  expect_identical(kv$a5_index, "1.0")
  if (requireNamespace("jsonlite", quietly = TRUE)) {
    index <- jsonlite::fromJSON(kv$a5pyramid)
    expect_named(index$levels, c("lod", "rows", "tiles"))
    expect_false(any(grepl("row_group", names(index))))
  }
  expect_match(kv$a5pyramid, '"format_version":1')
  tbl <- reader$ReadTable()
  expect_identical(hex(a5R::a5_cell_from_arrow(tbl$cell)), hex(a5_read_pyramid(path)$cell))
})
