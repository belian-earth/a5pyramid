
<!-- README.md is generated from README.Rmd. Please edit that file -->

# a5pyramid <a href="https://belian-earth.github.io/a5pyramid/"><img src="man/figures/logo.png" align="right" height="200" alt="a5pyramid website" /></a>

<!-- badges: start -->

[![Lifecycle:
experimental](https://img.shields.io/badge/lifecycle-experimental-orange.svg)](https://lifecycle.r-lib.org/articles/stages.html#experimental)
[![License: Apache
2.0](https://img.shields.io/badge/license-Apache%202.0-blue.svg)](https://www.apache.org/licenses/LICENSE-2.0)
[![R-CMD-check](https://github.com/belian-earth/a5pyramid/actions/workflows/R-CMD-check.yaml/badge.svg)](https://github.com/belian-earth/a5pyramid/actions/workflows/R-CMD-check.yaml)
[![Codecov test
coverage](https://codecov.io/gh/belian-earth/a5pyramid/graph/badge.svg)](https://app.codecov.io/gh/belian-earth/a5pyramid)
[![extendr](https://img.shields.io/badge/extendr-%5E0.9.0-276DC2)](https://extendr.rs/extendr/extendr_api/)
<!-- badges: end -->

a5pyramid builds level-of-detail pyramids of [A5](https://a5geo.org/)
cells and writes them, with the cells they summarise, to one Parquet
file laid out for the browser. A web map reads the file’s footer, picks
the level for its zoom and fetches only the tiles in view with HTTP
range requests, for example with
[hyparquet](https://github.com/hyparam/hyparquet) and deck.gl. No tile
server and no pre-cut tile directory are needed: one static file serves
every zoom.

It takes cells from [a5R](https://github.com/belian-earth/a5R) and
[a5px](https://github.com/belian-earth/a5px). The engine is a Rust crate
with no R dependency, in `src/rust/a5pyramid`, to be published on its
own.

|  |  |
|----|----|
| Build | `a5_pyramid()` builds the levels as a tibble; `a5_write_pyramid()` builds and writes them to Parquet in Rust, in parallel; `a5_pyramid_arrow()` returns the same tiles in memory as an Arrow table, without a file |
| Reduce | mean, sum, min, max, mode, or a representative leaf that keeps real combinations of values such as colours; a mean of codes, such as AEF embeddings, is taken on the values they decode to |
| Read | `a5_pyramid_info()` reads the index from the footer; `a5_read_pyramid()` reads levels within a bounding box, as a renderer does |
| Sets | `a5_write_pyramid_set()` splits a pyramid too large for one file into shards, each a small file around one coarse cell, with an index, `set.json`, so a browser opens only the shards in view |
| Format | One tile per Parquet row group (or Arrow record batch), with each tile’s level and bounding box indexed in the metadata; specified in [`FORMAT.md`](src/rust/a5pyramid/FORMAT.md) |

## Installation

a5pyramid is not on CRAN. Install the development version from GitHub:

``` r
# install.packages("pak")
pak::pak("belian-earth/a5pyramid")
```

Building from source needs a [Rust toolchain](https://rustup.rs/) with
`rustc` 1.88 or later.

## Example

`s2_santiago`, from a5view, holds 15,626 resolution-18 cells (about 22 m
across) of a cloud-free Sentinel-2 mosaic over Santiago, Chile, with the
blue, green, red and near-infrared bands. This writes a pyramid of it
with a representative leaf for the bands, so a coarse cell keeps the
colour of a real pixel rather than a blend, and the mean NDVI.

``` r
library(a5pyramid)

santiago <- a5view::s2_santiago
santiago$ndvi <- with(santiago, (B08 - B04) / (B08 + B04))

path <- tempfile(fileext = ".parquet")
a5_write_pyramid(
  santiago, path,
  aggregate = "rep_child",
  reducers = c(ndvi = "mean"),
  types = c(B02 = "uint16", B03 = "uint16", B04 = "uint16", B08 = "uint16", ndvi = "float32")
)
```

``` r
info <- a5_pyramid_info(path)
info
#> ── a5pyramid file 'filec01de20b7c3ad.parquet' ──────────────────────────────────
#> Size: 380.3 KB (footer 10.8 KB)
#> A5 index: 1.0
#> Leaves: resolution 18, 15,626 cells
#> LODs: 2-18 (17 levels)
#> Rows: 21,077
#> Tiles: 22 row groups (tile size 4096)
#> Columns: B02 <uint16, rep_child>, B03 <uint16, rep_child>, B04 <uint16,
#> rep_child>, B08 <uint16, rep_child>, ndvi <float32, mean>
#> Bbox: -70.6722, -33.4801, -70.6418, -33.4549
#> Written by: a5pyramid 0.0.0.9000, a5 crate 1.0.0
```

Every coarser level groups the leaves by the cell containing their
centres, so each level partitions the data and `ncell` counts the leaves
under each cell. A browser opening the file reads the footer only, here
11 KB, and then the tiles it draws. `a5_read_pyramid()` reads levels the
same way:

``` r
library(ggplot2)

levels <- a5_read_pyramid(path, lod = c(14, 16, 18))
ggplot(levels, aes(cell = cell, fill = a5view::cells_rgb(B04, B03, B02, range = c(300, 2800)))) +
  a5view::geom_a5() +
  facet_wrap(~ paste("LOD", lod)) +
  theme_void()
```

<img src="man/figures/README-levels-1.png" alt="" width="100%" />

## How it works

**Levels.** A cell at LOD `r` summarises the leaves whose centres it
contains (`a5R::a5_cell_to_supercell()`). Supercells do not chain: a
leaf’s supercell two resolutions up is often not the supercell of its
supercell one resolution up, so every level is grouped from the leaves
directly.

**Tiles.** Each level is split into tiles of at most `tile_size` rows,
one Parquet row group each. A tile is a run of consecutive cell ids, and
A5 ids follow a space-filling curve, so a run is a compact patch of the
globe. Levels are split down the A5 hierarchy only where a run holds
more cells than fit a tile, so tiles are coarse where data is sparse and
fine where it is dense, and every level stays sorted by cell id.
With `row_group_size`, a row group holds several tiles, each one data page
located through the Parquet page index, so an engine that scans whole row
groups, such as DuckDB over S3, makes fewer requests while a browser still
reads one tile at a time (format version 4).

**Index.** The file’s key-value metadata holds a JSON index of the
levels, the value columns, and the bounding box of every tile, computed
from the cells’ boundaries (antimeridian and poles included). Cell ids
are delta encoded, which takes them from 8 bytes per row to about 3, and
Snappy compression keeps the file readable by hyparquet with no extra
libraries. The test suite reads files with hyparquet under Node and
checks that only the footer and the requested tile are fetched.

## Performance

A pyramid of 439,276 resolution-16 cells over central Scotland, with
four `uint8` colour channels and a `float32` value, builds and writes in
about 0.55 s on 20 threads with either `mean` or `rep_child`
(`benchmarks/bench-write.R`). The file is 6.5 MB in 172 tiles, with a 78
KB footer. Results do not depend on the number of threads, which
`a5pyramid_threads()` sets.
