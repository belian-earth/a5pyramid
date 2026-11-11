# Codecs: a mean of codes taken on the values they stand for.

# The AEF decode, and the a5embeddings store's encoder: DuckDB's ROUND
# (half away from zero) of sign(v) * sqrt(|v|) * 127.5, clamped to int8.
aef_decode <- function(q) {
  t <- q / 127.5
  t * abs(t)
}
aef_encode <- function(v) {
  r <- sign(v) * sqrt(abs(v)) * 127.5
  pmin(127, pmax(-128, sign(r) * floor(abs(r) + 0.5)))
}

# The code of each cell's weighted mean of decoded codes at `lod`, by hex
# cell, the long way and exactly: with whole codes and weights the mean of
# q |q| is num / den, and its code is the largest k with
# (k - 1/2)^2 <= |num| / den, ties away from zero, decided in integers.
aef_means <- function(cells, codes, w, lod) {
  stopifnot(all(w == round(w)))
  g <- groups_at(cells, lod)
  num <- rowsum(w * codes * abs(codes), g)[, 1]
  den <- rowsum(w, g)[, 1]
  a <- abs(num)
  k <- floor(sqrt(a / den) + 0.5)
  k <- k - ((2 * k - 1)^2 * den > 4 * a & k > 0)
  k <- k + ((2 * k + 1)^2 * den <= 4 * a)
  stats::setNames(pmin(127, pmax(-128, sign(num) * k)), names(num))
}

codec_leaves <- function(seed = 1L, dim = 6L) {
  x <- make_leaves(seed)
  n <- nrow(x)
  # codes as in AEF data, -127 to 127, mostly small
  x$emb <- matrix(as.integer(pmax(-127, pmin(127, round(stats::rnorm(n * dim, 0, 45))))), n, dim)
  x$q <- x$emb[, 1]
  x
}

test_that("the exact means are the store's encoder of the decoded means, but at ties", {
  x <- codec_leaves()
  for (lod in c(11L, 13L)) {
    g <- groups_at(x$cell, lod)
    float <- rowsum(x$count * aef_decode(x$q), g)[, 1] / rowsum(x$count, g)[, 1]
    exact <- aef_means(x$cell, x$q, x$count, lod)
    expect_identical(unname(aef_encode(float[names(exact)])), unname(exact))
  }
})

test_that("a codec mean is the code of the mean of the decoded leaves", {
  x <- codec_leaves()
  p <- a5_pyramid(x, q, weights = "count", types = c(q = "int8"), codecs = c(q = "aef"),
                  lods = c(11, 13))
  plain <- a5_pyramid(x, q, weights = "count", types = c(q = "int8"), lods = c(11, 13))
  for (lod in c(11L, 13L)) {
    got <- level(p, lod)
    want <- aef_means(x$cell, x$q, x$count, lod)
    expect_identical(as.numeric(got$q), unname(want[hex(got$cell)]))
  }
  # the mean of the codes is not the code of the mean
  expect_false(identical(p$q, plain$q))
  # the leaves keep their codes
  leaves <- level(p, 15L)
  expect_identical(leaves$q, x$q[match(hex(leaves$cell), hex(x$cell))])
})

test_that("a mean vector is each element's mean, decoded with a codec", {
  x <- codec_leaves()
  sorted <- function(p) p[order(p$lod, hex(p$cell)), ]
  p <- sorted(a5_pyramid(x, emb, weights = "count", reducers = c(emb = "mean"),
                         codecs = c(emb = "aef"), lods = c(10, 12)))
  expect_true(is.matrix(p$emb))
  for (lod in c(10L, 12L)) {
    got <- p[p$lod == lod, ]
    for (e in seq_len(ncol(x$emb))) {
      want <- aef_means(x$cell, x$emb[, e], x$count, lod)
      expect_identical(as.numeric(got$emb[, e]), unname(want[hex(got$cell)]))
    }
  }
  # without a codec, the mean of the codes, rounded half to even
  m <- sorted(a5_pyramid(x, emb, weights = "count", reducers = c(emb = "mean"), lods = 12L))
  got <- m[m$lod == 12L, ]
  g <- groups_at(x$cell, 12L)
  for (e in seq_len(ncol(x$emb))) {
    num <- rowsum(x$count * x$emb[, e], g)
    want <- stats::setNames(round(num[, 1] / rowsum(x$count, g)[, 1]), rownames(num))
    expect_identical(as.numeric(got$emb[, e]), unname(want[hex(got$cell)]))
  }
  # a vector is its elements as scalar columns
  y <- x
  cols <- paste0("e", seq_len(ncol(x$emb)))
  for (e in seq_along(cols)) y[[cols[e]]] <- x$emb[, e]
  s <- sorted(a5_pyramid(y, tidyselect::all_of(cols), weights = "count", lods = c(10, 12),
                         types = setNames(rep("int8", length(cols)), cols),
                         codecs = setNames(rep("aef", length(cols)), cols)))
  expect_identical(unname(p$emb), unname(as.matrix(s[cols])))
})

test_that("a mean vector leaves the representatives to the rep_child columns", {
  x <- codec_leaves()
  x$red <- sample(0:255, nrow(x), replace = TRUE)
  x$emb2 <- x$emb[, 6:1]
  with <- a5_pyramid(x, red, emb, emb2, aggregate = "rep_child", types = c(red = "uint8"),
                     reducers = c(emb2 = "mean"), codecs = c(emb2 = "aef"), lods = 11L)
  without <- a5_pyramid(x, red, emb, aggregate = "rep_child", types = c(red = "uint8"), lods = 11L)
  expect_identical(with$red, without$red)
  expect_identical(with$emb, without$emb)
})

test_that("the index records codecs, through a file and a set", {
  x <- codec_leaves(dim = 8L)
  path <- tmp_parquet()
  a5_write_pyramid(x, path, value, emb, weights = "count", reducers = c(emb = "mean"),
                   codecs = c(emb = "aef"), tile_size = 300)
  info <- a5_pyramid_info(path)
  expect_identical(info$format_version, 3L)
  expect_identical(info$columns$reducer, c("mean", "mean"))
  expect_identical(info$columns$codec, c(NA, "aef"))
  expect_identical(info$columns$sd, c(NA_real_, NA_real_))
  expect_match(paste(cli::cli_fmt(print(info)), collapse = "\n"), "emb <int8, mean, aef>", fixed = TRUE)
  want <- a5_pyramid(x, emb, weights = "count", reducers = c(emb = "mean"), codecs = c(emb = "aef"))
  back <- a5_read_pyramid(path)
  key <- function(p) order(p$lod, hex(p$cell))
  expect_identical(unname(back$emb[key(back), ]), unname(want$emb[key(want), ]))

  dest <- withr::local_tempdir()
  unlink(dest, recursive = TRUE)
  tiers <- data.frame(lod_min = c(2, 13), lod_max = c(12, 15), shard_resolution = c(0, 9), tile_size = 500)
  a5_write_pyramid_set(x, dest, emb, weights = "count", tiers = tiers,
                       reducers = c(emb = "mean"), codecs = c(emb = "aef"))
  set <- a5_pyramid_set_info(dest)
  expect_identical(set$columns$codec, "aef")
  rows <- do.call(rbind, lapply(file.path(dest, set$shards$path), function(f) {
    r <- a5_read_pyramid(f)
    data.frame(cell = hex(r$cell), lod = r$lod, emb = apply(r$emb, 1, paste, collapse = ","))
  }))
  want <- a5_pyramid(x, emb, weights = "count", reducers = c(emb = "mean"), codecs = c(emb = "aef"),
                     lods = 2:14)
  want <- data.frame(cell = hex(want$cell), lod = want$lod, emb = apply(want$emb, 1, paste, collapse = ","))
  key <- function(d) d[order(d$lod, d$cell), ]
  expect_equal(key(rows), key(want), ignore_attr = TRUE)
})

test_that("Arrow vectors take codecs, as the store's TINYINT[64] does", {
  skip_if_not_installed("arrow")
  x <- codec_leaves(dim = 4L)
  want <- a5_pyramid(x, emb, weights = "count", reducers = c(emb = "mean"),
                     codecs = c(emb = "aef"), lods = 12L)
  rows <- lapply(seq_len(nrow(x)), function(i) x$emb[i, ])
  tbl <- arrow::arrow_table(
    cell = a5R::a5_cell_to_arrow(x$cell),
    emb = arrow::Array$create(rows, type = arrow::fixed_size_list_of(arrow::int8(), 4L)),
    count = arrow::Array$create(x$count, type = arrow::uint16())
  )
  got <- a5_pyramid(tbl, emb, weights = "count", reducers = c(emb = "mean"),
                    codecs = c(emb = "aef"), lods = 12L)
  key <- function(p) order(p$lod, hex(p$cell))
  expect_identical(unname(got$emb[key(got), ]), unname(want$emb[key(want), ]))
})

test_that("bad codecs are refused", {
  x <- codec_leaves()
  err <- function(..., msg = NULL) {
    expect_error(a5_pyramid(x, ...), msg, class = "a5pyramid_error_input")
  }
  err(q, emb, codecs = c(q = "sqrt"), msg = "must be one of")
  err(q, codecs = c(nope = "aef"), msg = "not a value column")
  # a vector is rep_child unless named mean
  err(emb, codecs = c(emb = "aef"), msg = "applies to")
  err(q, types = c(q = "int8"), reducers = c(q = "sum"), codecs = c(q = "aef"), msg = "applies to")
  err(q, codecs = c(q = "aef"), msg = "decodes \"int8\" codes")
  err(value, codecs = c(value = "aef"), msg = "decodes \"int8\" codes")
  err(emb, reducers = c(emb = "mean"), types = c(emb = "uint8"), codecs = c(emb = "aef"))
  err(emb, reducers = c(emb = "max"), msg = "rep_child")
})
