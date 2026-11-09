# Tiers over make_leaves() (res 15, 3 km around Edinburgh) with several
# shards in the finest tier.
set_tiers <- function() {
  data.frame(
    lod_min = c(2L, 9L, 13L),
    lod_max = c(8L, 12L, 15L),
    shard_resolution = c(0L, 7L, 10L),
    tile_size = c(100L, 200L, 300L)
  )
}

tmp_set <- function(env = parent.frame()) withr::local_tempfile(.local_envir = env)

read_set <- function(dest) {
  info <- a5_pyramid_set_info(dest)
  rows <- lapply(seq_len(nrow(info$shards)), function(i) {
    out <- a5_read_pyramid(file.path(dest, info$shards$path[i]))
    out$shard <- hex(info$shards$cell[i])
    out$tier <- info$shards$tier[i]
    out
  })
  list(info = info, rows = do.call(rbind, rows))
}

test_that("a set's shards hold the pyramid's rows, each in the shard containing its centre", {
  x <- make_leaves()
  types <- c(value = "float32", count = "uint8")
  dest <- tmp_set()
  out <- a5_write_pyramid_set(x, dest, value, count, tiers = set_tiers(), types = types,
                              aggregate = "rep_child", metadata = c(app = "a"))
  expect_identical(out, dest)

  s <- read_set(dest)
  built <- a5_pyramid(x, value, count, types = types, aggregate = "rep_child")
  key <- function(d) paste(d$lod, hex(d$cell))
  got <- s$rows[order(key(s$rows)), ]
  want <- built[order(key(built)), ]
  expect_identical(hex(got$cell), hex(want$cell))
  expect_identical(got$lod, want$lod)
  expect_identical(got$ncell, want$ncell)
  expect_identical(got$value, want$value)
  expect_identical(got$count, want$count)

  res <- set_tiers()$shard_resolution[s$rows$tier + 1L]
  owner <- vapply(seq_len(nrow(s$rows)), function(i) {
    hex(a5R::a5_cell_to_supercell(s$rows$cell[i], resolution = res[i]))
  }, character(1))
  expect_identical(owner, s$rows$shard)
  expect_gt(sum(s$info$shards$tier == 2L), 1L)
})

test_that("the set index describes its shards", {
  x <- make_leaves()
  dest <- tmp_set()
  a5_write_pyramid_set(x, dest, value, tiers = set_tiers(), metadata = list(app = "a", note = "b"))
  info <- a5_pyramid_set_info(dest)
  expect_s3_class(info, "a5pyramid_set_info")
  expect_identical(info$format_version, 2L)
  expect_identical(info$a5_index, a5R::a5_info()$index)
  expect_identical(info$data_resolution, 15L)
  expect_identical(info$metadata, c(app = "a", note = "b"))
  expect_identical(info$tiers$lod_min, set_tiers()$lod_min)
  expect_identical(info$tiers$tile_size, set_tiers()$tile_size)
  expect_identical(sum(info$tiers$shards), nrow(info$shards))
  expect_true(a5R::is_a5_cell(info$shards$cell))
  # the index reads the same through set.json
  expect_identical(a5_pyramid_set_info(file.path(dest, "set.json"))$shards, info$shards)

  for (i in seq_len(nrow(info$shards))) {
    sh <- info$shards[i, ]
    path <- file.path(dest, sh$path)
    fi <- a5_pyramid_info(path)
    expect_identical(fi$format_version, 2L)
    expect_identical(fi$shard$cell, hex(sh$cell))
    expect_identical(fi$shard$tier, sh$tier)
    expect_identical(fi$data_resolution, 15L)
    expect_identical(fi$lods, set_tiers()$lod_min[sh$tier + 1L]:set_tiers()$lod_max[sh$tier + 1L])
    expect_identical(sh$bytes, file.size(path))
    expect_identical(sh$footer_bytes, fi$footer_bytes)
    expect_identical(sh$rows, sum(fi$levels$rows))
    expect_identical(sh$tiles, nrow(fi$tiles))
    expect_true(all(fi$tiles$rows <= set_tiers()$tile_size[sh$tier + 1L]))
    # the shard's box holds every tile, coarse ones included
    expect_true(all(fi$tiles$xmin >= sh$xmin & fi$tiles$xmax <= sh$xmax &
                    fi$tiles$ymin >= sh$ymin & fi$tiles$ymax <= sh$ymax))
    expect_match(cli::cli_fmt(print(fi))[1], "a5pyramid shard")
  }
  printed <- cli::cli_fmt(print(info))
  expect_match(printed[1], "a5pyramid set")
  expect_true(any(grepl("Tier 2", printed)))
})

test_that("default tiers follow the rule", {
  t <- a5_pyramid_set_tiers(18)
  expect_identical(t$lod_min, c(2L, 13L, 16L))
  expect_identical(t$lod_max, c(12L, 15L, 18L))
  expect_identical(t$shard_resolution, c(0L, 3L, 6L))
  expect_identical(t$tile_size, rep(16384L, 3))
  t <- a5_pyramid_set_tiers(18, lods = c(8, 17))
  expect_identical(t$lod_min, c(8L, 16L))

  # without tiers, a set takes the default ones; make_leaves() is at 15
  x <- make_leaves()
  dest <- tmp_set()
  a5_write_pyramid_set(x, dest, value)
  info <- a5_pyramid_set_info(dest)
  expect_identical(info$tiers$lod_min, a5_pyramid_set_tiers(15)$lod_min)
  expect_identical(info$tiers$shard_resolution, a5_pyramid_set_tiers(15)$shard_resolution)
})

test_that("given tiers, the levels default to the LODs the tiers hold", {
  x <- make_leaves()
  dest <- tmp_set()
  tiers <- data.frame(lod_min = c(6L, 13L), lod_max = c(10L, 15L), shard_resolution = c(0L, 10L))
  a5_write_pyramid_set(x, dest, value, tiers = tiers)
  s <- read_set(dest)
  expect_identical(sort(unique(s$rows$lod)), c(6:10, 13:15))
  expect_identical(s$info$tiers$tile_size, c(16384L, 16384L))
})

test_that("bad tiers and occupied directories are refused", {
  x <- make_leaves()
  bad <- list(
    data.frame(lod_min = 2L, lod_max = 15L),
    data.frame(lod_min = 2L, lod_max = 15L, shard_resolution = 3L),
    data.frame(lod_min = c(2L, 8L), lod_max = c(8L, 15L), shard_resolution = c(0L, 6L)),
    data.frame(lod_min = 2L, lod_max = 15L, shard_resolution = -1L),
    data.frame(lod_min = 2L, lod_max = 15L, shard_resolution = 0L, tile_size = 0L)
  )
  for (tiers in bad) {
    dest <- tmp_set()
    expect_error(a5_write_pyramid_set(x, dest, value, tiers = tiers), class = "a5pyramid_error_input")
    expect_false(dir.exists(dest) && file.exists(file.path(dest, "set.json")))
  }
  # the leaves in no tier
  dest <- tmp_set()
  expect_error(
    a5_write_pyramid_set(x, dest, value, lods = 2:14,
                         tiers = data.frame(lod_min = 2L, lod_max = 14L, shard_resolution = 0L)),
    "LOD 15 is in no tier",
    class = "a5pyramid_error_input"
  )

  dest <- tmp_set()
  dir.create(dest)
  writeLines("x", file.path(dest, "other"))
  expect_error(a5_write_pyramid_set(x, dest, value), class = "a5pyramid_error_io")
  file <- tmp_parquet()
  writeLines("x", file)
  expect_error(a5_write_pyramid_set(x, file, value), class = "a5pyramid_error_io")
  expect_error(a5_pyramid_set_info(tempdir()), class = "a5pyramid_error_io")
})

test_that("a single file keeps format version 1 and no shard", {
  x <- make_leaves()
  path <- tmp_parquet()
  a5_write_pyramid(x, path, value)
  info <- a5_pyramid_info(path)
  expect_identical(info$format_version, 1L)
  expect_null(info$shard)
  expect_match(cli::cli_fmt(print(info))[1], "a5pyramid file")
})
