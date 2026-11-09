test_that("every level partitions the leaves", {
  x <- make_leaves()
  p <- a5_pyramid(x, value, lods = c(4L, 8L, 12L))

  expect_s3_class(p, "tbl_df")
  expect_named(p, c("cell", "lod", "ncell", "value"))
  expect_true(a5R::is_a5_cell(p$cell))
  expect_identical(unique(p$lod), c(4L, 8L, 12L, 15L))
  for (lod in unique(p$lod)) {
    l <- p[p$lod == lod, ]
    expect_identical(sum(l$ncell), nrow(x))
    expect_identical(unique(a5R::a5_get_resolution(l$cell)), lod)
    # ascending cell id within each level
    expect_false(is.unsorted(hex(l$cell), strictly = TRUE))
  }
  # the leaves come back sorted, values intact
  leaves <- level(p, 15L)
  ref <- x[order(hex(x$cell)), ]
  expect_identical(hex(leaves$cell), hex(ref$cell))
  expect_identical(leaves$value, ref$value)
  expect_true(all(leaves$ncell == 1L))
})

test_that("means and counts match grouping by supercell", {
  x <- make_leaves()
  p <- a5_pyramid(x, value, lods = c(9L, 12L))
  for (lod in c(9L, 12L)) {
    g <- groups_at(x$cell, lod)
    l <- level(p, lod)
    expect_setequal(hex(l$cell), unique(g))
    ref_mean <- tapply(x$value, g, mean)
    ref_n <- tapply(x$value, g, length)
    k <- hex(l$cell)
    expect_equal(l$value, as.vector(ref_mean[k]), tolerance = 1e-12)
    expect_identical(l$ncell, as.integer(ref_n[k]))
  }
})

test_that("sum, min, max and mode reduce each group", {
  x <- make_leaves()
  x$s <- x$value
  x$lo <- x$value
  x$hi <- x$value
  p <- a5_pyramid(
    x, s, lo, hi, class,
    lods = 10L,
    reducers = c(s = "sum", lo = "min", hi = "max", class = "mode")
  )
  l <- level(p, 10L)
  g <- groups_at(x$cell, 10L)
  k <- hex(l$cell)
  expect_equal(l$s, as.vector(tapply(x$value, g, sum)[k]), tolerance = 1e-12)
  expect_identical(l$lo, as.vector(tapply(x$value, g, min)[k]))
  expect_identical(l$hi, as.vector(tapply(x$value, g, max)[k]))
  mode_ref <- tapply(x$class, g, function(v) {
    tab <- table(v)
    as.integer(names(tab)[which.max(tab)]) # first maximum: the smallest value
  })
  expect_identical(l$class, as.vector(mode_ref[k]))
})

test_that("rep_child takes all its columns from the leaf nearest the scaled mean", {
  x <- make_leaves()
  x <- x[order(hex(x$cell)), ]
  x$big <- x$value * 1000 + stats::rnorm(nrow(x))
  p <- a5_pyramid(x, value, big, lods = 11L, aggregate = "rep_child")
  l <- level(p, 11L)
  pairs <- paste(x$value, x$big)
  expect_true(all(paste(l$value, l$big) %in% pairs))

  # the chosen leaf minimises the distance with each column scaled by its
  # sd; ties (any two-leaf group) go to the lowest cell id
  g <- groups_at(x$cell, 11L)
  sv <- stats::sd(x$value)
  sb <- stats::sd(x$big)
  for (i in seq_len(nrow(l))) {
    m <- x[g == hex(l$cell[i]), ]
    d <- ((m$value - mean(m$value)) / sv)^2 + ((m$big - mean(m$big)) / sb)^2
    best <- which(d - min(d) <= 1e-10 * max(min(d), 1e-300))[1]
    expect_identical(l$value[i], m$value[best])
  }
})

test_that("weights weight means, and the weights column is summed", {
  x <- make_leaves()
  x$w <- as.double(x$count)
  x$w[1:5] <- NA
  p <- a5_pyramid(x, value, w, lods = 10L, weights = "w")
  l <- level(p, 10L)
  g <- groups_at(x$cell, 10L)
  w0 <- ifelse(is.na(x$w), 0, x$w)
  ref <- tapply(seq_len(nrow(x)), g, function(i) sum(x$value[i] * w0[i]) / sum(w0[i]))
  k <- hex(l$cell)
  expect_equal(l$value, as.vector(ref[k]), tolerance = 1e-12)
  expect_equal(l$w, as.vector(tapply(x$w, g, sum, na.rm = TRUE)[k]))

  expect_error(a5_pyramid(x, weights = "nope"), class = "a5pyramid_error_input")
  x$w[6] <- -1
  expect_error(a5_pyramid(x, weights = "w"), class = "a5pyramid_error_input")
})

test_that("missing values are skipped and all-missing cells are NA", {
  x <- make_leaves()
  g <- groups_at(x$cell, 12L)
  x$value[g == g[1]] <- NA
  x$value[2] <- NA
  p <- a5_pyramid(x, value, lods = 12L)
  l <- level(p, 12L)
  expect_true(is.na(l$value[hex(l$cell) == g[1]]))
  ref <- tapply(x$value, g, mean, na.rm = TRUE)
  ref[is.nan(ref)] <- NA
  expect_equal(l$value, as.vector(ref[hex(l$cell)]), tolerance = 1e-12)

  # NA cells drop their rows
  y <- x
  y$cell[1:3] <- NA
  expect_identical(sum(level(a5_pyramid(y, value), 15L)$ncell), nrow(x) - 3L)
})

test_that("storage types round half to even and clamp", {
  cells <- cap_cells(res = 13L)
  n <- length(cells)
  x <- data.frame(row.names = seq_len(n))
  x$cell <- cells
  x$u8 <- rep(c(0L, 255L), length.out = n)
  x$f32 <- rep(0.1, n)
  x$big <- rep(c(-500, 500), length.out = n)
  p <- a5_pyramid(x, lods = 8L, types = c(u8 = "uint8", f32 = "float32", big = "int8"))
  expect_type(p$u8, "integer")
  expect_true(all(p$u8 >= 0L & p$u8 <= 255L))
  # means of 0 and 255 alternating: rounded the way round() does
  l <- level(p, 8L)
  g <- groups_at(x$cell, 8L)
  expect_identical(l$u8, as.integer(round(as.vector(tapply(x$u8, g, mean)[hex(l$cell)]))))
  f32 <- readBin(writeBin(0.1, raw(), size = 4), "double", size = 4)
  expect_identical(unique(p$f32), f32)
  expect_true(all(p$big[p$lod == 13L] %in% c(-128L, 127L)))
})

test_that("row order and thread count do not change the result", {
  x <- make_leaves()
  a <- a5_pyramid(x, aggregate = "rep_child", threads = 1)
  b <- a5_pyramid(x[rev(seq_len(nrow(x))), ], aggregate = "rep_child", threads = 4)
  expect_identical(hex(a$cell), hex(b$cell))
  expect_identical(a[-1], b[-1])
})

test_that("default LODs run from 2 up to the leaves", {
  p <- a5_pyramid(cap_cells(res = 12L))
  expect_identical(unique(p$lod), 2:12)
  expect_named(p, c("cell", "lod", "ncell"))
  coarse <- unique(a5R::a5_lonlat_to_cell(c(0, 20, 40), c(0, 10, 20), 2L))
  expect_identical(unique(a5_pyramid(coarse)$lod), 2L)
})

test_that("only numeric columns are carried by default", {
  x <- make_leaves()
  x$label <- "a"
  x$f <- factor("b")
  p <- a5_pyramid(x, lods = 10L)
  expect_named(p, c("cell", "lod", "ncell", "value", "count", "class"))
  expect_error(a5_pyramid(x, label), class = "a5pyramid_error_input")
  expect_named(a5_pyramid(x, value, lods = 10L), c("cell", "lod", "ncell", "value"))
})

test_that("bad input is refused with classed errors", {
  x <- make_leaves()
  cells <- x$cell
  expect_error(a5_pyramid(c(cells, cells[1])), "more than once", class = "a5pyramid_error_input")
  mixed <- c(cells, a5R::a5_cell_to_parent(cells[1], 10L))
  expect_error(a5_pyramid(mixed), "one resolution", class = "a5pyramid_error_input")
  cov <- a5R::a5_spherical_cap(a5R::a5_lonlat_to_cell(0, 0, 10L), 1000)
  expect_error(a5_pyramid(cov), class = "a5pyramid_error_input")
  expect_error(a5_pyramid(x, lods = 15L), class = "a5pyramid_error_input")
  expect_error(a5_pyramid(x, lods = c(3, 3)), class = "a5pyramid_error_input")
  expect_error(a5_pyramid(x, lods = 2.5), class = "a5pyramid_error_input")
  y <- x
  y$lod <- 1
  expect_error(a5_pyramid(y), "cannot be named", class = "a5pyramid_error_input")
  expect_error(a5_pyramid(x, types = c(value = "int64")), class = "a5pyramid_error_input")
  expect_error(a5_pyramid(x, types = c(nope = "uint8")), class = "a5pyramid_error_input")
  expect_error(a5_pyramid(x, reducers = c(value = "median")), class = "a5pyramid_error_input")
  expect_error(a5_pyramid(x, reducers = "sum"), class = "a5pyramid_error_input")
  expect_error(a5_pyramid(x, aggregate = "sum"))
  expect_error(a5_pyramid(list(1)), class = "a5pyramid_error_input")
  expect_error(a5_pyramid(data.frame(a = 1)), class = "a5pyramid_error_input")
  expect_error(a5_pyramid(cells, value), class = "a5pyramid_error_input")
  expect_error(a5_pyramid(a5R::a5_cell(NA_character_)), class = "a5pyramid_error_input")
  expect_error(a5_pyramid(x, threads = 0), class = "a5pyramid_error_input")
})

test_that("rep_child's standard deviations are recorded, and given ones are used", {
  x <- make_leaves()
  x$red <- sample(0:255, nrow(x), replace = TRUE)
  x$green <- sample(0:255, nrow(x), replace = TRUE)
  path <- tmp_parquet()
  a5_write_pyramid(x, path, red, green, count, aggregate = "rep_child", reducers = c(count = "sum"))
  cols <- a5_pyramid_info(path)$columns
  expect_equal(cols$sd[1:2], c(stats::sd(x$red), stats::sd(x$green)))
  expect_true(is.na(cols$sd[3]))

  computed <- a5_pyramid(x, red, green, aggregate = "rep_child")
  given <- a5_pyramid(x, red, green, aggregate = "rep_child", rep_sd = c(red = cols$sd[1], green = cols$sd[2]))
  expect_identical(given, computed)
  # all but ignoring green: each cell takes the leaf nearest in red
  red_only <- a5_pyramid(x, red, green, aggregate = "rep_child", rep_sd = c(green = 1e9), lods = 12L)
  l12 <- level(red_only, 12L)
  g <- groups_at(x$cell, 12L)
  mean_red <- tapply(x$red, g, mean)
  for (k in seq_len(nrow(l12))) {
    reds <- x$red[g == hex(l12$cell[k])]
    expect_equal(abs(l12$red[k] - mean_red[[hex(l12$cell[k])]]), min(abs(reds - mean_red[[hex(l12$cell[k])]])))
  }
})

test_that("bad rep_sd is refused", {
  x <- make_leaves()
  err <- function(...) expect_error(a5_pyramid(x, value, count, aggregate = "rep_child", reducers = c(count = "sum"), ...),
                                    class = "a5pyramid_error_input")
  err(rep_sd = c(count = 1))
  err(rep_sd = c(value = 0))
  err(rep_sd = c(value = NA))
  err(rep_sd = c(value = Inf))
  err(rep_sd = 1)
  err(rep_sd = c(value = "1"))
  expect_error(a5_pyramid(x, value, rep_sd = c(value = 1)), "No column is reduced", class = "a5pyramid_error_input")
})
