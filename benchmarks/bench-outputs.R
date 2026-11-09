# Time the three outputs of one pyramid: a tibble (a5_pyramid), an Arrow
# table (a5_pyramid_arrow) and a Parquet file (a5_write_pyramid).
#
#   Rscript benchmarks/bench-outputs.R [resolution] [threads]
#
# Needs a5pyramid installed (R CMD INSTALL, a release build) and arrow.
suppressMessages({
  library(a5R)
  library(a5pyramid)
})
args <- commandArgs(trailingOnly = TRUE)
res <- if (length(args) >= 1) as.integer(args[1]) else 16L
threads <- if (length(args) >= 2) as.integer(args[2]) else a5pyramid_threads()

leaves <- a5_uncompact(a5_polygon_to_cells(wk::rct(-3.6, 55.7, -2.6, 56.2), resolution = res), resolution = res)
n <- length(leaves)
set.seed(1)
x <- data.frame(cell = leaves, r = sample(0:255, n, TRUE), g = sample(0:255, n, TRUE),
                b = sample(0:255, n, TRUE), value = runif(n))
types <- c(r = "uint8", g = "uint8", b = "uint8", value = "float32")

time <- function(expr) {
  gc()
  system.time(expr)[["elapsed"]]
}
path <- tempfile(fileext = ".parquet")
cat(sprintf("%s leaves at resolution %d on %d threads\n", format(n, big.mark = ","), res, threads))
for (agg in c("mean", "rep_child")) {
  t_tbl <- time(p <- a5_pyramid(x, aggregate = agg, types = types, threads = threads))
  t_arw <- time(tbl <- a5_pyramid_arrow(x, aggregate = agg, types = types, threads = threads))
  t_pq <- time(a5_write_pyramid(x, path, aggregate = agg, types = types, threads = threads))
  cat(sprintf("%-9s tibble %.2fs  arrow %.2fs  parquet %.2fs | %d rows, %d tiles\n",
              agg, t_tbl, t_arw, t_pq, tbl$num_rows, tbl$cell$num_chunks))
}
