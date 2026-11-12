# Read levels of a pyramid file

Reads the rows of the tiles at `lod` whose bounding boxes meet `bbox`,
as a renderer does for a view: the footer first, then only those tiles'
row groups, or their data pages where row groups hold several tiles.
Selection is by tile, so rows of a selected tile that lie outside `bbox`
are returned too; filter the cells afterwards for an exact cut.

## Usage

``` r
a5_read_pyramid(src, lod = NULL, bbox = NULL)
```

## Arguments

- src:

  Path of a pyramid Parquet file written by
  [`a5_write_pyramid()`](https://belian-earth.github.io/a5pyramid/reference/a5_write_pyramid.md).

- lod:

  Integer vector of the LODs to read, each one of the file's (see
  [`a5_pyramid_info()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid_info.md)),
  or `NULL` (default) for every level.

- bbox:

  `NULL` (default) for everywhere, or `c(xmin, ymin, xmax, ymax)` in WGS
  84 degrees. For a box across the antimeridian give `xmin > xmax`, such
  as `c(179, -1, -179, 1)`.

## Value

A tibble with the columns of
[`a5_pyramid()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid.md),
in file order: ascending LOD, then cell id; with an `owner` column (an
[a5R::a5_cell](https://belian-earth.github.io/a5R/reference/a5_cell.html))
and the weights column where the file has them. A file without `lod` or
`ncell` columns gives its level and 1.

## See also

[`a5_pyramid_info()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid_info.md)
for the file's levels and tiles.

## Examples

``` r
library(a5R)
centre <- a5_lonlat_to_cell(-3.19, 55.95, resolution = 10)
cells <- a5_uncompact(a5_spherical_cap(centre, 3000), resolution = 15)
path <- tempfile(fileext = ".parquet")
a5_write_pyramid(data.frame(cell = cells, value = runif(length(cells))), path)

a5_read_pyramid(path, lod = 12)
#> # A tibble: 22 × 4
#>    cell               lod ncell value
#>    <a5_cell>        <int> <int> <dbl>
#>  1 6316007800000000    12    18 0.493
#>  2 63bb939800000000    12    16 0.413
#>  3 63bb93c800000000    12    16 0.561
#>  4 63bb940800000000    12    46 0.592
#>  5 63bb941800000000    12    62 0.546
#>  6 63bb942800000000    12    66 0.514
#>  7 63bb943800000000    12    66 0.440
#>  8 63bb944800000000    12    48 0.433
#>  9 63bb945800000000    12    66 0.515
#> 10 63bb946800000000    12    62 0.512
#> # ℹ 12 more rows
a5_read_pyramid(path, lod = 15, bbox = c(-3.2, 55.94, -3.18, 55.96))
#> # A tibble: 1,024 × 4
#>    cell               lod ncell  value
#>    <a5_cell>        <int> <int>  <dbl>
#>  1 63bb940020000000    15     1 0.741 
#>  2 63bb940060000000    15     1 0.206 
#>  3 63bb9400a0000000    15     1 0.613 
#>  4 63bb9400e0000000    15     1 0.0553
#>  5 63bb940120000000    15     1 0.0253
#>  6 63bb940160000000    15     1 0.0309
#>  7 63bb9401a0000000    15     1 0.472 
#>  8 63bb9401e0000000    15     1 0.642 
#>  9 63bb940220000000    15     1 0.0231
#> 10 63bb940260000000    15     1 0.895 
#> # ℹ 1,014 more rows
```
