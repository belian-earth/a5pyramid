# Time a5_write_pyramid() on the leaves of central Scotland (439,276 cells at
# resolution 16) with four uint8 colour channels and a float32 value, for the
# mean and rep_child aggregates. Each time is the median of three runs.
#
#   Rscript benchmarks/bench-write.R [resolution] [threads]
#
# Needs a5pyramid installed (R CMD INSTALL, a release build).
suppressMessages({
  library(a5R)
  library(a5pyramid)
})
args <- commandArgs(trailingOnly = TRUE)
res <- if (length(args) >= 1) as.integer(args[1]) else 16L
threads <- if (length(args) >= 2) as.integer(args[2]) else a5pyramid_threads()

region <- a5_polygon_to_cells(wk::rct(-3.6, 55.7, -2.6, 56.2), resolution = res)
leaves <- a5_uncompact(region, resolution = res)
n <- length(leaves)
set.seed(1)
df <- data.frame(row.names = seq_len(n))
df$cell <- leaves
df$r <- sample(0:255, n, TRUE)
df$g <- sample(0:255, n, TRUE)
df$b <- sample(0:255, n, TRUE)
df$a <- 255L
df$value <- runif(n)
types <- c(r = "uint8", g = "uint8", b = "uint8", a = "uint8", value = "float32")

cat(sprintf("%s leaves at resolution %d on %d threads\n\n",
            format(n, big.mark = ","), res, threads))
cat(sprintf("%-10s %8s %10s %8s %10s\n", "aggregate", "time_s", "file_MB", "tiles", "footer_KB"))
for (agg in c("mean", "rep_child")) {
  path <- tempfile(fileext = ".parquet")
  times <- vapply(1:3, function(i) {
    gc()
    system.time(a5_write_pyramid(df, path, aggregate = agg, types = types, threads = threads))[["elapsed"]]
  }, numeric(1))
  info <- a5_pyramid_info(path)
  cat(sprintf("%-10s %8.2f %10.2f %8d %10.1f\n", agg, stats::median(times),
              info$bytes / 1e6, nrow(info$tiles), info$footer_bytes / 1e3))
}
