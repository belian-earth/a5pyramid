# Write a pyramid as a set of shard files

Builds the pyramid of
[`a5_pyramid()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid.md)
and writes it as a *pyramid set*: a directory of small pyramid files,
*shards*, and an index, `set.json`. Use it where one file would be too
large to open in a browser. A browser reads a file's whole footer before
it draws anything, and the footer grows by a few hundred bytes a tile,
so a single file of 150 million leaves has a footer of about 20 MB. In a
set, a view opens only the few shards around it.

## Usage

``` r
a5_write_pyramid_set(
  x,
  dest,
  ...,
  lods = NULL,
  tiers = NULL,
  aggregate = c("mean", "rep_child"),
  reducers = NULL,
  weights = NULL,
  types = NULL,
  rep_sd = NULL,
  compression = c("snappy", "zstd", "none"),
  metadata = NULL,
  threads = NULL
)
```

## Arguments

- x:

  A data frame with one
  [a5R::a5_cell](https://belian-earth.github.io/a5R/reference/a5_cell.html)
  column, an
  [a5R::a5_cell](https://belian-earth.github.io/a5R/reference/a5_cell.html)
  vector, or Arrow data with one `uint64` column of cells: an Arrow
  `Table`, `RecordBatch`, `RecordBatchReader`, `Dataset` or query from
  the arrow package. The cells must share one resolution and appear once
  each; rows with `NA` cells are dropped. An
  [a5R::a5_covering](https://belian-earth.github.io/a5R/reference/a5_covering.html)
  holds cells at mixed resolutions: convert it with
  [`a5R::a5_uncompact()`](https://belian-earth.github.io/a5R/reference/a5_compaction.html)
  first. Arrow data is read into the engine batch by batch without
  passing through R, so a `Dataset` over Parquet files (from
  [`arrow::open_dataset()`](https://arrow.apache.org/docs/r/reference/open_dataset.html))
  builds without R holding the leaves.

- dest:

  Directory to write the set to. It must not exist or be empty, so a set
  never mixes with another's files.

- ...:

  \<[`tidy-select`](https://tidyselect.r-lib.org/reference/language.html)\>
  Value columns to carry into the pyramid. Default: every numeric column
  other than the cells. For an Arrow `Dataset` that includes its
  partitioning columns, such as `year` from `year=2025/` directories, so
  name the value columns there.

- lods:

  Integer vector of the coarser LODs to build. Default: every LOD the
  `tiers` hold below the leaves, or, without `tiers`, every resolution
  from 2 up to the leaves'.

- tiers:

  A data frame with one row per tier and columns `lod_min` and `lod_max`
  (the tier's LODs, inclusive), `shard_resolution` (from 0 up to
  `lod_min`) and optionally `tile_size` (default 16384). Between them
  the tiers must hold every level, the leaves included, and each must
  hold at least one. Default:
  [`a5_pyramid_set_tiers()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid_set_tiers.md).

- aggregate:

  The reducer for every value column: `"mean"` (default) or
  `"rep_child"`. See "Reducers".

- reducers:

  Named character vector of reducers for single columns, overriding
  `aggregate`, such as `c(count = "sum", class = "mode")`.

- weights:

  Name of a numeric column of non-negative leaf weights for `"mean"`,
  `"mode"` and `"rep_child"`, such as the `npix` column from
  `a5px::a5_read_raster()`. `NA` weights count as 0, and leaves with
  weight 0 do not count. Default `NULL`: every leaf counts once.

- types:

  Named character vector of storage types for single columns, such as
  `c(red = "uint8", ndvi = "float32")`. See "Column types".

- rep_sd:

  Named numeric vector of standard deviations for `"rep_child"` columns,
  such as `c(red = 60, green = 60)`. Default `NULL`: each column's
  standard deviation over the leaves, or 1 where that is 0 or undefined
  (a constant column). That depends on every leaf, so pyramids built
  from different parts of a region choose representatives differently
  even where their cells are the same; fix the standard deviations (from
  a whole-region build's `a5_pyramid_info()$columns$sd`, say) for builds
  whose cells must agree.

- compression:

  `"snappy"` (default), `"zstd"` or `"none"`. hyparquet decodes snappy
  itself; zstd needs the `hyparquet-compressors` package in the browser.

- metadata:

  Named character vector or list of strings to add to the file's
  key-value metadata, such as a renderer's settings as JSON. The keys
  `"a5pyramid"`, `"a5_index"` and `"ARROW:schema"` are reserved.

- threads:

  Number of threads, or `NULL` (default) for
  [`a5pyramid_threads()`](https://belian-earth.github.io/a5pyramid/reference/a5pyramid_threads.md).

## Value

`dest`, invisibly.

## Tiers and shards

The LODs are split into *tiers*, each a range of LODs with a *shard
resolution*. A cell of a tier belongs to the shard keyed by the cell at
the shard resolution containing its centre
([`a5R::a5_cell_to_supercell()`](https://belian-earth.github.io/a5R/reference/a5_spatial_hierarchy.html)),
so every cell is in exactly one shard. Each shard is one file holding
its tier's LODs for the cells it owns, at
`tier=<t>/shard=<cell>.parquet`. The levels are built from all the
leaves before the pyramid is split, so a coarse cell near a shard's edge
summarises every leaf under it, whichever shard holds them.

[`a5_pyramid_set_tiers()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid_set_tiers.md)
gives the default tiers: three LODs a tier from the leaves down, each
keyed 12 resolutions coarser than its finest LOD, and the remaining LODs
in one tier keyed at resolution 0. A shard full of data then holds about
22 million rows, and a map view meets at most a few shards of the tier
it draws at any zoom.

## Files

A shard is an a5pyramid file in format version 2: as a file from
[`a5_write_pyramid()`](https://belian-earth.github.io/a5pyramid/reference/a5_write_pyramid.md),
with the index also recording the shard, and with `data_resolution` the
leaves' resolution even in shards of coarser tiers.
[`a5_pyramid_info()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid_info.md)
and
[`a5_read_pyramid()`](https://belian-earth.github.io/a5pyramid/reference/a5_read_pyramid.md)
read shards. A shard can hold no cells at some LOD of its tier, never at
all of them; shards without cells are not written. `set.json` lists the
tiers and every shard with its path, rows, tiles, size, footer size and
bounding box, and is written last, so a set with an index is complete. A
write that fails or is interrupted removes the shards it wrote, and
`dest` if it created it, so it can be run again. The format is specified
in `FORMAT.md` in the package source.

## See also

[`a5_pyramid_set_info()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid_set_info.md)
to read the index,
[`a5_write_pyramid()`](https://belian-earth.github.io/a5pyramid/reference/a5_write_pyramid.md)
for a single file.

## Examples

``` r
library(a5R)
centre <- a5_lonlat_to_cell(-3.19, 55.95, resolution = 10)
cells <- a5_uncompact(a5_spherical_cap(centre, 3000), resolution = 15)
x <- data.frame(cell = cells, value = runif(length(cells)))

dest <- tempfile()
tiers <- data.frame(
  lod_min = c(2, 13), lod_max = c(12, 15),
  shard_resolution = c(0, 10), tile_size = c(4096, 4096)
)
a5_write_pyramid_set(x, dest, tiers = tiers)
info <- a5_pyramid_set_info(dest)
info
#> ── a5pyramid set file28a41566c636 ──────────────────────────────────────────────
#> Size: 24.7 KB in 6 shards
#> A5 index: 1.0
#> Leaves: resolution 15
#> Tier 0: LODs 2-12, shards at resolution 0: 2 shards, 46 rows, 6.6 KB (footers
#> up to 3.5 KB)
#> Tier 1: LODs 13-15, shards at resolution 10: 4 shards, 1,380 rows, 18.1 KB
#> (footers up to 1.4 KB)
#> Columns: value <float64, mean>
#> Bbox: -21, 49.9853, 15, 71.6383
#> Written by: a5pyramid 0.0.0.9000, a5 crate 1.0.0
info$shards
#> # A tibble: 6 × 11
#>    tier cell             path   rows tiles bytes footer_bytes   xmin  ymin  xmax
#>   <int> <a5_cell>        <chr> <dbl> <int> <dbl>        <dbl>  <dbl> <dbl> <dbl>
#> 1     0 0200000000000000 tier…     1     1  1228         1093 -21     52.2 15   
#> 2     0 1200000000000000 tier…    45    10  5361         3460 -13.2   50.0 -1.52
#> 3     1 6316008000000000 tier…   162     3  3200         1388  -3.22  56.0 -3.16
#> 4     1 63bb948000000000 tier…   977     3  9202         1398  -3.22  55.9 -3.14
#> 5     1 63bb958000000000 tier…    32     3  2153         1375  -3.15  56.0 -3.13
#> 6     1 63bb968000000000 tier…   209     3  3575         1386  -3.18  55.9 -3.12
#> # ℹ 1 more variable: ymax <dbl>
```
