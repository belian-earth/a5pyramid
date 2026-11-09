# Inspect a pyramid set

Reads the index of a set written by
[`a5_write_pyramid_set()`](https://belian-earth.github.io/a5pyramid/reference/a5_write_pyramid_set.md),
`set.json`, without opening any shard. A renderer reads it first, then
the footers of the shards its view meets.

## Usage

``` r
a5_pyramid_set_info(src)
```

## Arguments

- src:

  The set's directory, or its `set.json`.

## Value

A list of class `a5pyramid_set_info`:

- `path`: the set's directory.

- `format_version`, `generator`, `a5_crate` and `a5_index`, as in
  [`a5_pyramid_info()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid_info.md).

- `data_resolution`: the leaves' resolution.

- `bbox`: `c(xmin, ymin, xmax, ymax)` of every shard.

- `columns`: a tibble of the value columns' `name`, `type`, `reducer`
  and `sd`, the standard deviation that scaled a `"rep_child"` column (1
  where the leaves' standard deviation is 0 or undefined; `NA` for other
  reducers and older files).

- `tiers`: a tibble with one row per tier: `tier` (from 0), `lod_min`,
  `lod_max`, `shard_resolution`, `tile_size`, and its `shards`, `rows`
  and `bytes`.

- `shards`: a tibble with one row per shard: `tier`, `cell` (the shard
  cell, an
  [a5R::a5_cell](https://belian-earth.github.io/a5R/reference/a5_cell.html)),
  `path` (relative to the set's directory), `rows`, `tiles`, `bytes`,
  `footer_bytes` (what a reader fetches to open it) and `xmin`, `ymin`,
  `xmax`, `ymax`.

- `metadata`: the set's metadata, a named character vector.

## See also

[`a5_pyramid_info()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid_info.md)
for one shard.

## Examples

``` r
library(a5R)
centre <- a5_lonlat_to_cell(-3.19, 55.95, resolution = 10)
cells <- a5_uncompact(a5_spherical_cap(centre, 3000), resolution = 15)
dest <- tempfile()
a5_write_pyramid_set(data.frame(cell = cells, value = runif(length(cells))), dest)
a5_pyramid_set_info(dest)$tiers
#> # A tibble: 2 × 8
#>    tier lod_min lod_max shard_resolution tile_size shards  rows bytes
#>   <int>   <int>   <int>            <int>     <int>  <int> <dbl> <dbl>
#> 1     0       2      12                0     16384      2    46  6591
#> 2     1      13      15                3     16384      1  1380 11857
```
