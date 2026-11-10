# Inspect a pyramid file

Reads the footer of a file written by
[`a5_write_pyramid()`](https://belian-earth.github.io/a5pyramid/reference/a5_write_pyramid.md),
its index and Parquet layout, without reading any rows. This is what a
browser fetches first, so `footer_bytes` is the cost of opening the file
there.

## Usage

``` r
a5_pyramid_info(src)
```

## Arguments

- src:

  Path of a pyramid Parquet file.

## Value

A list of class `a5pyramid_info`:

- `path`, `bytes` (file size), `footer_bytes` (Parquet footer, index
  included).

- `format_version`, `generator`, `a5_crate` (the a5 crate that built the
  file) and `a5_index` (the A5 index version of its ids).

- `data_resolution`: the leaves' resolution. `lods`: every level's LOD,
  ascending, the leaves last.

- `tile_size`: the most rows in a tile.

- `bbox`: `c(xmin, ymin, xmax, ymax)` of the leaves.

- `columns`: a tibble of the value columns' `name`, `type`, `reducer`
  and `sd`, the standard deviation that scaled a `"rep_child"` column (1
  where the leaves' standard deviation is 0 or undefined; `NA` for other
  reducers and older files), and `dim`, the elements per cell of a
  vector column (`NA` for scalar columns).

- `levels`: a tibble of each level's `lod`, `rows` and number of
  `tiles`.

- `tiles`: a tibble with one row per tile, which is one row group of the
  file: `tile` (its 0-based index, the row group's index as Parquet
  readers count), `lod`, `rows`, `bytes` (compressed size, what a reader
  fetches) and `xmin`, `ymin`, `xmax`, `ymax`.

- `metadata`: the file's other key-value metadata, a named character
  vector.

- `shard`: for a shard of a pyramid set (format version 2, written by
  [`a5_write_pyramid_set()`](https://belian-earth.github.io/a5pyramid/reference/a5_write_pyramid_set.md)),
  a list of its `tier`, shard `cell` (hex) and its shard `resolution`;
  `NULL` for a single file.

## Details

Longitudes follow the a5R stack's convention: a box across the
antimeridian has `xmax` above 180, as
[`a5R::a5_bbox()`](https://belian-earth.github.io/a5R/reference/a5_bbox.html)
reports. The file itself stores such a box with `west > east`, as
GeoJSON does.

## See also

[`a5_read_pyramid()`](https://belian-earth.github.io/a5pyramid/reference/a5_read_pyramid.md)
to read rows.

## Examples

``` r
library(a5R)
centre <- a5_lonlat_to_cell(-3.19, 55.95, resolution = 10)
cells <- a5_uncompact(a5_spherical_cap(centre, 3000), resolution = 15)
path <- tempfile(fileext = ".parquet")
a5_write_pyramid(data.frame(cell = cells, value = runif(length(cells))), path)

info <- a5_pyramid_info(path)
info
#> ── a5pyramid file file29306d291993.parquet ─────────────────────────────────────
#> Size: 17.1 KB (footer 4.6 KB)
#> A5 index: 1.0
#> Leaves: resolution 15, 1,024 cells
#> LODs: 2-15 (14 levels)
#> Rows: 1,426
#> Tiles: 14 row groups (tile size 4096)
#> Columns: value <float64, mean>
#> Bbox: -3.2194, 55.9355, -3.127, 56.0338
#> Written by: a5pyramid 0.0.0.9000, a5 crate 1.0.0
info$levels
#> # A tibble: 14 × 3
#>      lod  rows tiles
#>    <int> <dbl> <int>
#>  1     2     1     1
#>  2     3     1     1
#>  3     4     1     1
#>  4     5     1     1
#>  5     6     2     1
#>  6     7     2     1
#>  7     8     2     1
#>  8     9     3     1
#>  9    10     4     1
#> 10    11     7     1
#> 11    12    22     1
#> 12    13    76     1
#> 13    14   280     1
#> 14    15  1024     1
```
