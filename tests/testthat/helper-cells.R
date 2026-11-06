# Cells at resolution `res` within `radius` metres of a point.
cap_cells <- function(lon = -3.19, lat = 55.95, radius = 3000, res = 15L) {
  centre <- a5R::a5_lonlat_to_cell(lon, lat, resolution = 10L)
  a5R::a5_uncompact(a5R::a5_spherical_cap(centre, radius), resolution = res)
}

hex <- function(x) a5R::a5_u64_to_hex(x)

# Leaves with a few kinds of value column, in a scrambled row order.
make_leaves <- function(seed = 1L, ...) {
  set.seed(seed)
  cells <- cap_cells(...)
  n <- length(cells)
  x <- data.frame(row.names = seq_len(n))
  x$cell <- cells
  x$value <- stats::rnorm(n, 10, 3)
  x$count <- sample(1:9, n, replace = TRUE)
  x$class <- sample(c(1L, 2L, 2L, 3L), n, replace = TRUE)
  x[sample.int(n), , drop = FALSE]
}

# Reference: every leaf's group at `lod`, as a5R computes it.
groups_at <- function(cells, lod) hex(a5R::a5_cell_to_supercell(cells, resolution = lod))

level <- function(p, lod) {
  out <- p[p$lod == lod, , drop = FALSE]
  out[order(hex(out$cell)), , drop = FALSE]
}

tmp_parquet <- function(env = parent.frame()) withr::local_tempfile(fileext = ".parquet", .local_envir = env)
