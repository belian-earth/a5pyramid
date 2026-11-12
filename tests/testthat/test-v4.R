# Format version 4: list vectors, a weights column, levels written from
# rows with owner and without lod or ncell, and shard tables.

v4_leaves <- function(seed = 1L) {
  set.seed(seed)
  cells <- cap_cells()
  n <- length(cells)
  x <- data.frame(cell = cells, n_pixels = sample(3:7, n, replace = TRUE))
  x$emb <- matrix(sample(-128:127, n * 16, replace = TRUE), n, 16)
  x
}

aef_mean <- function(x, ...) {
  a5_pyramid(x, emb, ..., weights = "n_pixels", weights_type = "uint32",
             reducers = c(emb = "mean"), codecs = c(emb = "aef"))
}

tmp_set <- function(env = parent.frame()) withr::local_tempfile(.local_envir = env)

schema_names <- function(path) {
  skip_if_not_installed("arrow")
  names(arrow::ParquetFileReader$create(path)$GetSchema())
}

test_that("summed weights are kept at every level", {
  x <- v4_leaves()
  p <- aef_mean(x, lods = 11:14)
  expect_true("n_pixels" %in% names(p))
  expect_false("n_pixels" %in% names(a5_pyramid(x, emb, weights = "n_pixels")))
  totals <- tapply(p$n_pixels, p$lod, sum)
  expect_true(all(totals == sum(x$n_pixels)))
  # a cell's weights are its leaves'
  l13 <- level(p, 13)
  want <- tapply(x$n_pixels, groups_at(x$cell, 13), sum)
  expect_equal(as.numeric(l13$n_pixels), as.numeric(want[hex(l13$cell)]))
  expect_error(a5_pyramid(x, emb, weights_type = "uint32"), class = "a5pyramid_error_input")
  expect_error(a5_pyramid(x, emb, weights = "n_pixels", weights_type = "int64"), class = "a5pyramid_error_input")
})

test_that("list vectors write and read back as fixed binary ones do", {
  x <- v4_leaves()
  a <- tmp_parquet()
  b <- tmp_parquet()
  a5_write_pyramid(x, a, emb, reducers = c(emb = "mean"), codecs = c(emb = "aef"), tile_size = 300)
  a5_write_pyramid(x, b, emb, reducers = c(emb = "mean"), codecs = c(emb = "aef"), tile_size = 300,
                   storage = c(emb = "list"), row_group_size = 1200)
  expect_identical(a5_read_pyramid(b), a5_read_pyramid(a))
  ib <- a5_pyramid_info(b)
  expect_identical(ib$format_version, 4L)
  expect_identical(ib$columns$storage, "list")
  expect_identical(a5_pyramid_info(a)$columns$storage, "fixed_binary")
  expect_error(a5_write_pyramid(x, b, emb, n_pixels, storage = c(n_pixels = "list")), class = "a5pyramid_error_input")
  expect_error(a5_write_pyramid(x, b, emb, storage = c(emb = "tuple")), class = "a5pyramid_error_input")
  skip_if_not_installed("duckdb")
  con <- DBI::dbConnect(duckdb::duckdb())
  on.exit(DBI::dbDisconnect(con, shutdown = TRUE))
  got <- DBI::dbGetQuery(con, sprintf("SELECT any_value(typeof(emb)) AS t, min(len(emb)) AS lo, max(len(emb)) AS hi FROM read_parquet('%s')", b))
  expect_identical(got$t, "TINYINT[]")
  expect_identical(c(got$lo, got$hi), c(16, 16))
})

test_that("Arrow list columns, as DuckDB returns TINYINT[], are vectors", {
  skip_if_not_installed("arrow")
  x <- v4_leaves()
  tbl <- arrow::arrow_table(
    cell = a5R::a5_cell_to_arrow(x$cell),
    n_pixels = x$n_pixels,
    emb = arrow::Array$create(lapply(seq_len(nrow(x)), function(i) x$emb[i, ]), type = arrow::list_of(arrow::int8()))
  )
  expect_identical(aef_mean(tbl, lods = 12)[-1], aef_mean(x, lods = 12)[-1])
  ragged <- tbl
  short <- lapply(seq_len(nrow(x)), function(i) x$emb[i, ])
  short[[3]] <- short[[3]][-1]
  ragged$emb <- arrow::Array$create(short, type = arrow::list_of(arrow::int8()))
  expect_error(aef_mean(ragged, lods = 12), "15 elements at row 3", class = "a5pyramid_error_input")
  expect_error(aef_mean(arrow::as_record_batch_reader(tbl), lods = 12), "cannot be read twice",
               class = "a5pyramid_error_input")
})

test_that("a level written from rows carries owner, weights and only the columns it needs", {
  x <- v4_leaves()
  p <- aef_mean(x, lods = 13)
  l13 <- level(p, 13)
  key <- a5R::a5_cell_to_supercell(x$cell[1], resolution = 9)
  path <- tmp_parquet()
  rows <- l13[sample.int(nrow(l13)), ]
  a5_write_pyramid_level(rows, path, emb, data_resolution = 15, ncell = "ncell", weights = "n_pixels",
                         reducers = c(emb = "mean"), codecs = c(emb = "aef"), storage = c(emb = "list"),
                         shard = list(tier = 2, cell = key, resolution = 9), owner = TRUE, tile_size = 50,
                         row_group_size = 200, compression = "snappy")
  info <- a5_pyramid_info(path)
  expect_identical(info$format_version, 4L)
  expect_true(info$owner)
  expect_identical(info$weights, list(name = "n_pixels", type = "uint32"))
  expect_identical(info$shard$tier, 2L)
  expect_identical(info$lods, 13L)
  back <- a5_read_pyramid(path)
  expect_identical(hex(back$cell), hex(l13$cell))
  expect_identical(back$emb, l13$emb)
  expect_identical(back$ncell, l13$ncell)
  expect_equal(back$n_pixels, l13$n_pixels)
  expect_identical(hex(back$owner), hex(a5R::a5_cell_to_supercell(l13$cell, resolution = 9)))
  expect_identical(schema_names(path), c("cell", "ncell", "owner", "n_pixels", "emb"))

  # the leaves: no lod, no ncell
  leaves <- tmp_parquet()
  a5_write_pyramid_level(x, leaves, emb, weights = "n_pixels", reducers = c(emb = "mean"), codecs = c(emb = "aef"),
                         storage = c(emb = "list"))
  expect_identical(schema_names(leaves), c("cell", "n_pixels", "emb"))
  lf <- a5_read_pyramid(leaves)
  expect_true(all(lf$ncell == 1L) && all(lf$lod == 15L))
  expect_identical(lf$emb, level(p, 15)$emb)
})

test_that("rows that do not make a level are refused", {
  x <- v4_leaves()
  p <- aef_mean(x, lods = 13)
  l13 <- level(p, 13)
  key <- a5R::a5_cell_to_supercell(x$cell[1], resolution = 9)
  path <- tmp_parquet()
  w <- function(...) a5_write_pyramid_level(l13, path, emb, data_resolution = 15, ...)
  # coarse rows need ncell; owner needs a shard; the shard cell must be at its resolution
  expect_error(w(), "need their ncell", class = "a5pyramid_error_input")
  expect_error(w(ncell = "ncell", owner = TRUE), class = "a5pyramid_error_input")
  expect_error(w(ncell = "ncell", shard = list(tier = 0, cell = key, resolution = 8)), class = "a5pyramid_error_input")
  expect_error(w(ncell = "nope"), class = "a5pyramid_error_input")
  expect_error(a5_write_pyramid_level(rbind(l13, l13[1, ]), path, emb, data_resolution = 15, ncell = "ncell"),
               "more than once", class = "a5pyramid_error_input")
  expect_error(a5_write_pyramid_level(p, path, emb, data_resolution = 15, ncell = "ncell"),
               "not an A5 cell|one resolution|resolution", class = "a5pyramid_error_input")
  expect_false(file.exists(path))
})

test_that("a set's index can be a shard table", {
  x <- v4_leaves()
  dest <- tmp_set()
  tiers <- data.frame(lod_min = c(2L, 13L), lod_max = c(12L, 15L), shard_resolution = c(0L, 10L),
                      tile_size = c(100L, 100L), row_group_size = c(NA, 400L))
  a5_write_pyramid_set(x, dest, emb, weights = "n_pixels", weights_type = "uint32", reducers = c(emb = "mean"),
                       codecs = c(emb = "aef"), storage = c(emb = "list"), tiers = tiers, index = "parquet")
  expect_true(file.exists(file.path(dest, "set.parquet")))
  expect_false(file.exists(file.path(dest, "set.json")))
  set <- a5_pyramid_set_info(dest)
  expect_identical(set$format_version, 4L)
  expect_identical(set$weights$name, "n_pixels")
  expect_identical(set$columns$storage, "list")
  expect_identical(a5_pyramid_set_info(file.path(dest, "set.parquet"))$shards, set$shards)
  rows <- sum(vapply(set$shards$path, function(p) nrow(a5_read_pyramid(file.path(dest, p))), integer(1)))
  expect_identical(rows, nrow(aef_mean(x, lods = 2:14)))
})

test_that("shard tables list empty shards and their writer's columns", {
  skip_if_not_installed("arrow")
  x <- v4_leaves()
  dir <- withr::local_tempdir()
  key <- a5R::a5_cell_to_supercell(x$cell[1], resolution = 9)
  a5_write_pyramid_level(x, file.path(dir, "a.parquet"), emb, shard = list(tier = 0, cell = key, resolution = 9))
  info <- a5_pyramid_info(file.path(dir, "a.parquet"))
  row <- function(cell, path, box) {
    out <- data.frame(tier = 0L, path = path, rows = 0, tiles = 0L, bytes = 0, footer_bytes = 0,
                      xmin = box[1], ymin = box[2], xmax = box[3], ymax = box[4], build = "b1", year = 2021L)
    out$cell <- cell
    out
  }
  full <- row(key, "a.parquet", unname(info$bbox))
  full[c("rows", "tiles", "bytes", "footer_bytes")] <- list(sum(info$tiles$rows), nrow(info$tiles), info$bytes, info$footer_bytes)
  far <- a5R::a5_lonlat_to_cell(c(0, 179.99), c(0, 0), resolution = 9)
  # an empty shard, and one whose box crosses the antimeridian
  cross <- row(far[2], "a.parquet", c(179.5, -0.5, 180.5, 0.5))
  cross[c("rows", "tiles", "bytes", "footer_bytes")] <- full[c("rows", "tiles", "bytes", "footer_bytes")]
  shards <- rbind(full, row(far[1], NA_character_, rep(NA_real_, 4)), cross)
  shards <- shards[order(hex(shards$cell)), ]
  tiers <- data.frame(lod_min = 15L, lod_max = 15L, shard_resolution = 9L)
  table <- file.path(dir, "set.parquet")
  a5_write_pyramid_set_table(shards, table, template = file.path(dir, "a.parquet"), tiers = tiers)
  set <- a5_pyramid_set_info(dir)
  expect_identical(set$format_version, 4L)
  s <- set$shards
  expect_identical(hex(s$cell), hex(shards$cell))
  expect_identical(s$path, shards$path)
  expect_identical(s$build, rep("b1", 3))
  expect_identical(s$year, rep(2021L, 3))
  expect_equal(s$xmax, shards$xmax)
  expect_identical(set$tiers$shards, 2L)
  # stored unwrapped
  raw <- arrow::read_parquet(table)
  expect_equal(raw$east[match(hex(far[2]), hex(shards$cell))], 180.5)

  bad <- shards[c(2, 1, 3), ]
  expect_error(a5_write_pyramid_set_table(bad, table, template = file.path(dir, "a.parquet"), tiers = tiers),
               "out of order", class = "a5pyramid_error_input")
  bad <- shards
  bad$rows[is.na(bad$path)] <- 3
  expect_error(a5_write_pyramid_set_table(bad, table, template = file.path(dir, "a.parquet"), tiers = tiers),
               "must be empty", class = "a5pyramid_error_input")
})
