skip_if_not_installed("arrow")

# Leaves at resolution 14 around a centre, a 4-element AEF vector each and
# integer weights.
partial_leaves <- function(seed = 1) {
  set.seed(seed)
  centre <- a5R::a5_lonlat_to_cell(-3.19, 55.95, resolution = 9)
  cells <- a5R::a5_uncompact(a5R::a5_spherical_cap(centre, 5000), resolution = 14)
  emb <- matrix(sample(-128:127, length(cells) * 4, replace = TRUE), ncol = 4)
  tibble::tibble(cell = cells, embedding = emb, n = as.double(1 + sample(0:8, length(cells), replace = TRUE)))
}

# The merged rows of one LOD as a data frame comparable with a5_pyramid()'s.
merged_level <- function(m, lod) {
  m <- as.data.frame(m)
  m <- m[m$lod == lod, ]
  m <- m[order(m$cell), ]
  list(cell = a5R::a5_u64_to_hex(a5R::a5_cell_from_arrow(arrow::Array$create(m$cell, type = arrow::uint64()))),
       ncell = as.integer(m$ncell), n = as.double(m$n),
       codes = do.call(rbind, lapply(m$embedding, as.integer)))
}

built_level <- function(p, lod) {
  p <- p[p$lod == lod, ]
  list(cell = a5R::a5_u64_to_hex(p$cell), ncell = as.integer(p$ncell), n = as.double(p$n),
       codes = unname(p$embedding))
}

test_that("merged partials of separate parts are the levels of one build", {
  x <- partial_leaves()
  lods <- 10:12
  built <- a5_pyramid(x, embedding, lods = lods, reducers = c(embedding = "mean"), codecs = c(embedding = "aef"),
                      weights = "n", weights_type = "uint32")
  by11 <- a5R::a5_cell_to_supercell(x$cell, 11)
  part <- vctrs::vec_group_id(by11) %% 3L
  parts <- lapply(split(seq_len(nrow(x)), part), function(i) {
    a5_pyramid_partials(x[i, ], embedding, lods = lods, key_resolution = 8, weights = "n",
                        codecs = c(embedding = "aef"))
  })
  m <- a5_pyramid_merge_partials(do.call(arrow::concat_tables, unname(parts)))
  for (lod in lods) expect_identical(merged_level(m, lod), built_level(built, lod), label = paste("LOD", lod))
})

test_that("partials keep their metadata through Parquet files and merge from a dataset", {
  x <- partial_leaves(2)
  half <- a5R::a5_cell_to_supercell(x$cell, 10) < a5R::a5_cell_to_supercell(x$cell[1], 10)
  dir <- withr::local_tempdir()
  for (h in c(TRUE, FALSE)) {
    p <- a5_pyramid_partials(x[half == h, ], embedding, lods = c(9, 12), key_resolution = c(6, 8), weights = "n",
                             codecs = c(embedding = "aef"))
    arrow::write_parquet(p, file.path(dir, paste0(h, ".parquet")))
  }
  m <- a5_pyramid_merge_partials(arrow::open_dataset(dir))
  built <- a5_pyramid(x, embedding, lods = c(9, 12), reducers = c(embedding = "mean"), codecs = c(embedding = "aef"),
                      weights = "n", weights_type = "uint32")
  expect_identical(merged_level(m, 12), built_level(built, 12))
  expect_identical(merged_level(m, 9), built_level(built, 9))
  # keys select one shard
  k <- a5R::a5_cell_from_arrow(m$key$chunk(0))
  one <- unique(k[as.integer(as.vector(m$lod)) == 12L])[1]
  s <- a5_pyramid_merge_partials(arrow::open_dataset(dir), keys = one)
  expect_true(s$num_rows > 0)
  expect_true(all(a5R::a5_u64_to_hex(a5R::a5_cell_from_arrow(s$key$chunk(0))) == a5R::a5_u64_to_hex(one)))
  # and write as a level
  out <- withr::local_tempfile(fileext = ".parquet")
  rows <- s[, c("cell", "ncell", "n", "embedding")]
  a5_write_pyramid_level(rows, out, embedding, data_resolution = 14, ncell = "ncell", weights = "n",
                         reducers = c(embedding = "mean"), codecs = c(embedding = "aef"),
                         shard = list(tier = 0, cell = one, resolution = 8))
  expect_equal(sum(a5_pyramid_info(out)$tiles$rows), s$num_rows)
})

test_that("unweighted partials divide by the leaves", {
  x <- partial_leaves(3)
  built <- a5_pyramid(x, embedding, lods = 11, reducers = c(embedding = "mean"), codecs = c(embedding = "aef"))
  p <- a5_pyramid_partials(x, embedding, lods = 11, key_resolution = 8, codecs = c(embedding = "aef"))
  expect_false("n" %in% names(p))
  m <- as.data.frame(a5_pyramid_merge_partials(p))
  m <- m[order(m$cell), ]
  expect_identical(do.call(rbind, lapply(m$embedding, as.integer)), unname(built$embedding[built$lod == 11, ]))
})

test_that("what does not merge is refused", {
  x <- partial_leaves()
  x$score <- runif(nrow(x))
  expect_error(a5_pyramid_partials(x, embedding, score, lods = 11, key_resolution = 8), class = "a5pyramid_error")
  x$n[1] <- 0
  expect_error(a5_pyramid_partials(x, embedding, lods = 11, key_resolution = 8, weights = "n"),
               "weigh more than zero")
  x$n[1] <- 1
  expect_error(a5_pyramid_partials(x, embedding, lods = 11, key_resolution = 12), "key resolution")
  expect_error(a5_pyramid_partials(x, embedding, key_resolution = 8), "lods")
  expect_error(a5_pyramid_merge_partials(arrow::arrow_table(x = 1)), "partials")
  expect_error(a5_pyramid_merge_partials(1), "Arrow data")
})
