# Build and write a pyramid with a5pyramid and with a5view's R
# implementation (0.3.0.9000), on the same leaves and columns: RGBA colour
# channels and a float value, as a5view prepares them.
#
#   Rscript benchmarks/bench-vs-a5view.R [resolution] [threads]
#
# Needs a5pyramid installed (R CMD INSTALL, a release build) and a5view.
suppressMessages({
  library(a5R)
  library(a5pyramid)
})
args <- commandArgs(trailingOnly = TRUE)
res <- if (length(args) >= 1) as.integer(args[1]) else 16L
threads <- if (length(args) >= 2) as.integer(args[2]) else a5pyramid_threads()

# central Scotland: 439,276 cells at resolution 16
region <- a5_polygon_to_cells(wk::rct(-3.6, 55.7, -2.6, 56.2), resolution = res)
leaves <- a5_uncompact(region, resolution = res)
n <- length(leaves)
set.seed(1)
df <- data.frame(row.names = seq_len(n))
df$cell <- leaves
df$`_fill_r` <- sample(0:255, n, TRUE)
df$`_fill_g` <- sample(0:255, n, TRUE)
df$`_fill_b` <- sample(0:255, n, TRUE)
df$`_fill_a` <- 255L
df$`_fill_value` <- runif(n)
types <- c(`_fill_r` = "uint8", `_fill_g` = "uint8", `_fill_b` = "uint8",
           `_fill_a` = "uint8", `_fill_value` = "float32")

time <- function(expr) {
  gc()
  t <- system.time(expr)[["elapsed"]]
  t
}
ns <- asNamespace("a5view")
cat(sprintf("%s leaves at resolution %d; a5pyramid on %d threads\n\n",
            format(n, big.mark = ","), res, threads))
cat(sprintf("%-10s %-10s %8s %8s %10s %12s\n", "engine", "aggregate", "time_s", "speedup", "file_MB", "row_groups"))
for (agg in c("mean", "rep_child")) {
  f_view <- tempfile(fileext = ".parquet")
  t_view <- time({
    p <- ns$build_a5_pyramid(leaves, df, res, 1L, agg)
    ns$serialise_pyramid_to_parquet(p$data, f_view, TRUE, TRUE, FALSE)
  })
  rg_view <- arrow::ParquetFileReader$create(f_view)$num_row_groups

  f_ours <- tempfile(fileext = ".parquet")
  t_ours <- time(a5_write_pyramid(df, f_ours, aggregate = agg, types = types, threads = threads))
  info <- a5_pyramid_info(f_ours)

  cat(sprintf("%-10s %-10s %8.2f %8s %10.2f %12d\n", "a5view", agg, t_view, "", file.size(f_view) / 1e6, rg_view))
  cat(sprintf("%-10s %-10s %8.2f %7.1fx %10.2f %12d\n", "a5pyramid", agg, t_ours, t_view / t_ours,
              info$bytes / 1e6, nrow(info$row_groups)))
}
cat(sprintf("\nfooter: %.1f KB for %d row groups\n", info$footer_bytes / 1e3, nrow(info$row_groups)))
