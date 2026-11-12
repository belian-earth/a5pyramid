# Write a set's shard table

Writes the index of a set whose shards were written one at a time, with
[`a5_write_pyramid_level()`](https://belian-earth.github.io/a5pyramid/reference/a5_write_pyramid_level.md)
or by another writer, as a Parquet table: one row per shard, the set's
own fields in its footer (format version 4). A reader filters the rows
by tier and box with row-group statistics, and the table can carry
columns of its writer's beside the format's, such as a year or a build
name, so a store's manifest can be its set's index. A shard may be
listed with no file, as empty.

## Usage

``` r
a5_write_pyramid_set_table(
  shards,
  dest,
  template,
  tiers,
  metadata = NULL,
  compression = c("snappy", "zstd", "none")
)
```

## Arguments

- shards:

  A data frame with one row per shard, by `tier` and then `cell`: `tier`
  (from 0), `cell` (an
  [a5R::a5_cell](https://belian-earth.github.io/a5R/reference/a5_cell.html)
  at the tier's shard resolution), `path` (the file, relative to the
  set's directory with `/` separators; `NA` for an empty shard), `rows`,
  `tiles`, `bytes`, `footer_bytes` and `xmin`, `ymin`, `xmax`, `ymax`
  (the box of every tile, `xmax` above 180 across the antimeridian, as
  [`a5_pyramid_info()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid_info.md)
  gives it; `NA` for an empty shard).
  [`a5_pyramid_info()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid_info.md)
  gives a file's values. Any other columns are written after these.

- dest:

  Path of the table to write, usually `set.parquet` in the set's
  directory.

- template:

  Path of one of the shards: the set's columns, weights, leaves'
  resolution and a5 crate are taken from its index.

- tiers:

  A data frame of the tiers, as for
  [`a5_write_pyramid_set()`](https://belian-earth.github.io/a5pyramid/reference/a5_write_pyramid_set.md).

- metadata:

  Named character vector of the set's metadata.

- compression:

  `"snappy"` (default), `"zstd"` or `"none"`. hyparquet decodes snappy
  itself; zstd needs the `hyparquet-compressors` package in the browser.

## Value

`dest`, invisibly.

## See also

[`a5_pyramid_set_info()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid_set_info.md)
reads it back.

## Examples

``` r
library(a5R)
centre <- a5_lonlat_to_cell(-3.19, 55.95, resolution = 10)
cells <- a5_uncompact(a5_spherical_cap(centre, 3000), resolution = 15)
dir <- tempfile()
dir.create(dir)
key <- a5_cell_to_supercell(cells[1], resolution = 9)
a5_write_pyramid_level(data.frame(cell = cells, value = runif(length(cells))),
                       file.path(dir, "leaves.parquet"), value,
                       shard = list(tier = 0, cell = key, resolution = 9))
info <- a5_pyramid_info(file.path(dir, "leaves.parquet"))
shards <- data.frame(
  tier = 0L, path = "leaves.parquet", rows = sum(info$tiles$rows),
  tiles = nrow(info$tiles), bytes = info$bytes, footer_bytes = info$footer_bytes,
  xmin = info$bbox[["xmin"]], ymin = info$bbox[["ymin"]],
  xmax = info$bbox[["xmax"]], ymax = info$bbox[["ymax"]]
)
shards$cell <- key
tiers <- data.frame(lod_min = 15, lod_max = 15, shard_resolution = 9)
a5_write_pyramid_set_table(shards, file.path(dir, "set.parquet"),
                           template = file.path(dir, "leaves.parquet"), tiers = tiers)
a5_pyramid_set_info(dir)$shards
#> # A tibble: 1 × 11
#>    tier cell             path    rows tiles bytes footer_bytes  xmin  ymin  xmax
#>   <int> <a5_cell>        <chr>  <dbl> <int> <dbl>        <dbl> <dbl> <dbl> <dbl>
#> 1     0 63bb920000000000 leave…  1024     1  6311          638 -3.22  55.9 -3.13
#> # ℹ 1 more variable: ymax <dbl>
```
