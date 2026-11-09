# make a hex logo: the pyramid of one A5 cell in eastern Sabah, built with
# a5pyramid from AlphaEarth Foundations embeddings and drawn as a stack of
# levels, the leaves at the base and the cell itself on top
library(devtools)
library(ggplot2)
load_all()

# ----- read embeddings -----

# Bands A09, A10 and A11 of the 2025 AEF embeddings, the bands of the
# a5-pyramids demo, at A5 resolution 14 (about 400 m cells) under one
# resolution-9 cell (about 15 km across) south-west of Sandakan. One AEF tile
# covers it, found in the index at
# https://data.source.coop/tge-labs/aef/v1/annual/aef_index.parquet.
# AEF: CC-BY 4.0, produced by Google and Google DeepMind.

top_res <- 9L
leaf_res <- 14L
top <- a5R::a5_lonlat_to_cell(117.933, 5.644, resolution = top_res)

outline <- wk::wk_coords(a5R::a5_cell_to_boundary(top))
bbox <- c(
  min(outline$x) - 0.01,
  min(outline$y) - 0.01,
  max(outline$x) + 0.01,
  max(outline$y) + 0.01
)

tile <- paste0(
  "https://data.source.coop/tge-labs/aef/v1/annual/2025/50N/",
  "xi6jj12yek9aykfje-0000008192-0000008192.tiff"
)
leaves <- a5px::a5_read_raster(
  tile,
  resolution = leaf_res,
  bands = 10:12,
  bbox = bbox,
  io_concurrency = 16L
)
# the leaves of the top cell: those whose centre lies in it
leaves <- leaves[a5R::a5_cell_to_supercell(leaves$cell, top_res) == top, ]

# ----- build the pyramid -----

# Every coarser level up to the top cell. rep_child keeps the colour of a
# real leaf at each coarse cell rather than a blend.
pyr <- a5_pyramid(
  leaves,
  A09, A10, A11,
  lods = top_res:(leaf_res - 1L),
  aggregate = "rep_child"
)

# ----- colours -----

# The demo's stretch of each band's codes (its 2nd and 98th percentiles over
# Sabah) to red, green and blue, slightly more saturated: the label sits on
# black, so nothing needs darkening for it. Tops at 90% brightness, sides
# at 50%.
stretch <- function(v, lo, hi) pmin(pmax((v - lo) / (hi - lo), 0), 1)
hsv_mat <- rgb2hsv(
  stretch(pyr$A09, -43, 69),
  stretch(pyr$A10, -60, 34),
  stretch(pyr$A11, 22, 72),
  maxColorValue = 1
)
shade <- function(v) hsv(hsv_mat[1, ], pmin(hsv_mat[2, ] * 1.15, 1), hsv_mat[3, ] * v)
pyr$top_fill <- shade(0.9)
pyr$side_fill <- shade(0.5)

# ----- stack the levels -----

# Cell outlines in kilometres around the top cell's centre.
centre <- unclass(a5R::a5_cell_to_lonlat(top))
local_km <- function(cells) {
  xy <- wk::wk_coords(a5R::a5_cell_to_boundary(cells))
  data.frame(
    id = xy$feature_id,
    x = (xy$x - centre$x) * cos(centre$y * pi / 180) * 111.32,
    y = (xy$y - centre$y) * 110.57
  )
}

# Each level is a plate seen from above at an angle: turned so the top
# cell's long axis runs across, flattened, raised `rise` above the one below
# and scaled by `taper` per level, so the levels narrow to the top cell.
# Coarse cells at the edge hold only some of their leaves (supercells nest
# by centre, not by outline); those with fewer than half are left out, so
# each plate keeps the top cell's outline.
taper <- 0.8
rise <- 2.6
thickness <- 0.35
flatten <- 0.38

top_km <- local_km(top)
axis <- eigen(stats::cov(cbind(top_km$x, top_km$y)))$vectors[, 1]
turn <- -atan2(axis[2], axis[1])

lods <- sort(unique(pyr$lod), decreasing = TRUE)
polys <- list()
for (k in seq_along(lods)) {
  level <- pyr[pyr$lod == lods[k], ]
  full <- 4^(leaf_res - lods[k])
  level <- level[lods[k] == leaf_res | level$ncell >= full / 2, ]
  xy <- local_km(level$cell)
  xy$x <- xy$x - mean(xy$x)
  xy$y <- xy$y - mean(xy$y)
  scale <- taper^(k - 1)
  sx <- (xy$x * cos(turn) - xy$y * sin(turn)) * scale
  sy <- (xy$x * sin(turn) + xy$y * cos(turn)) * scale * flatten + (k - 1) * rise
  # sides first, then tops: each side is the hull of a cell's top face and
  # that face moved down by the plate's thickness
  for (i in unique(xy$id)) {
    w <- xy$id == i
    hx <- c(sx[w], sx[w])
    hy <- c(sy[w], sy[w] - thickness)
    h <- grDevices::chull(hx, hy)
    polys[[length(polys) + 1L]] <- data.frame(
      x = hx[h], y = hy[h], fill = level$side_fill[i], line = NA
    )
  }
  for (i in unique(xy$id)) {
    w <- xy$id == i
    polys[[length(polys) + 1L]] <- data.frame(
      x = sx[w], y = sy[w], fill = level$top_fill[i], line = "#74ac9071"
    )
  }
}
polys <- do.call(rbind, Map(function(p, g) cbind(p, group = g), polys, seq_along(polys)))

# Into the frame [-1, 1]: the stack 1.25 tall with its top at 0.78, the
# label below it.
k <- 1.25 / diff(range(polys$y))
polys$x <- (polys$x - mean(range(polys$x))) * k
polys$y <- (polys$y - max(polys$y)) * k + 0.78

# ----- make plot -----

ggp <- ggplot(polys, aes(x, y, group = group)) +
  geom_polygon(aes(fill = fill, colour = line), linewidth = 0.25) +
  scale_fill_identity() +
  scale_colour_identity(na.value = NA) +
  annotate(
    "text",
    x = 0,
    y = -0.6,
    label = "a5pyramid",
    size = 12,
    colour = "#ffffffff",
    family = "Manjari Thin"
  ) +
  coord_equal(xlim = c(-1, 1), ylim = c(-1, 1), expand = FALSE) +
  theme_void() +
  theme(plot.background = element_rect(fill = "black", colour = NA))

ggp

tf <- fs::file_temp(ext = "png")
ggsave(tf, ggp, width = 100, height = 100, units = "mm", dpi = 300)

t2 <- fs::file_temp(ext = "png")
cropcircles::crop_hex(
  tf,
  to = t2,
  border_size = 10,
  border_colour = "#74ac90ff",
  bg_fill = "#000000ff"
)

usethis::use_logo(t2)
