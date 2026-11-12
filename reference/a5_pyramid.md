# Build a level-of-detail pyramid of A5 cells

`a5_pyramid()` aggregates A5 cells at one resolution, the *leaves*, to
coarser levels of detail (LODs) and returns every level as rows of one
tibble.
[`a5_write_pyramid()`](https://belian-earth.github.io/a5pyramid/reference/a5_write_pyramid.md)
builds the same pyramid and writes it to a Parquet file laid out for
browser rendering.

## Usage

``` r
a5_pyramid(
  x,
  ...,
  lods = NULL,
  aggregate = c("mean", "rep_child"),
  reducers = NULL,
  weights = NULL,
  types = NULL,
  rep_sd = NULL,
  codecs = NULL,
  weights_type = NULL,
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

- ...:

  \<[`tidy-select`](https://tidyselect.r-lib.org/reference/language.html)\>
  Value columns to carry into the pyramid. Default: every numeric column
  other than the cells. For an Arrow `Dataset` that includes its
  partitioning columns, such as `year` from `year=2025/` directories, so
  name the value columns there.

- lods:

  Integer vector of the coarser LODs to build, each below the leaves'
  resolution; the leaves are always kept. Default: every resolution from
  2 up to the leaves'. Resolutions 0 and 1 hold the 12 and 60 largest
  cells, which no map view draws.

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

- codecs:

  Named character vector of codecs for `"mean"` columns whose values are
  codes, such as `c(embedding = "aef")`. See "Codecs". Default `NULL`:
  every column stores its values.

- weights_type:

  `NULL` (default), or a storage type (see "Column types") in which to
  keep each row's summed `weights` at every level, as a column named as
  the weights, beside the value columns and not one of them: a reader
  that merges rows of one cell weights them by it (format version 4 in a
  file). Needs `weights`.

- threads:

  Number of threads, or `NULL` (default) for
  [`a5pyramid_threads()`](https://belian-earth.github.io/a5pyramid/reference/a5pyramid_threads.md).

## Value

A tibble with one row per cell of every level, in ascending LOD and then
cell id order:

- `cell`: the cell, an
  [a5R::a5_cell](https://belian-earth.github.io/a5R/reference/a5_cell.html).

- `lod`: its LOD (A5 resolution), an integer.

- `ncell`: the number of leaves it summarises (1 for the leaves).

- with `weights_type`, the summed weights, named as the weights column.

- the value columns, integer for the integer types up to `"uint16"` and
  double otherwise.

## Levels

A cell at LOD `r` summarises the leaves whose centres it contains, as
given by
[`a5R::a5_cell_to_supercell()`](https://belian-earth.github.io/a5R/reference/a5_spatial_hierarchy.html),
so each level partitions the leaves, and `ncell` counts the leaves in
each cell. Supercells do not chain: a leaf's supercell two resolutions
up is often not the supercell of its supercell one resolution up. Each
level is therefore grouped from the leaves directly, never from the
level below, and matches `a5px::a5_aggregate()` of the leaves to that
resolution.

## Reducers

Each value column is reduced over the leaves under each coarser cell by
its reducer, `aggregate` unless `reducers` names another:

- `"mean"`: the mean, weighted by `weights` when given. For a column of
  codes with a codec (see "Codecs"), the code of the mean of the decoded
  values.

- `"sum"`, `"min"`, `"max"`: the sum, minimum or maximum.

- `"mode"`: the most frequent value, by total weight when `weights` is
  given, ties going to the smallest value. For category codes.

- `"rep_child"`: the value of the representative leaf, the leaf closest
  to the cell's mean over all `"rep_child"` columns, each divided by its
  standard deviation so that units do not matter. Every `"rep_child"`
  column takes its value from the same leaf, so a coarse cell keeps a
  combination of values that occurs in the data, such as an RGB colour,
  rather than a blend. The standard deviations are computed over the
  leaves unless `rep_sd` fixes them, and are recorded in the index
  ([`a5_pyramid_info()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid_info.md)).

Missing values are skipped, and a cell whose leaves are all missing is
`NA`. The `weights` column, when selected, is summed unless `reducers`
says otherwise. Results do not depend on the number of threads.

## Vector columns

A *vector column* holds a fixed number of 8-bit values per cell, such as
an embedding: an integer matrix column of a data frame (one row per
cell), or a fixed-size list of `int8` or `uint8` (or fixed-size binary,
read as `uint8` unless `types` says `"int8"`, as arrow reads a version-3
file's vectors) in Arrow data, as DuckDB writes `TINYINT[64]`. Its type
is `"int8"` (default for a matrix) or `"uint8"`. It is reduced by
`"rep_child"` whatever `aggregate` says: every element takes part in the
distance to the cell's mean, scaled by one standard deviation over all
the elements (fixed by `rep_sd` like any other column), so a coarse cell
holds the whole vector of one leaf. Named `"mean"` in `reducers`, it is
reduced element by element instead: each element of a coarse cell is the
(weighted) mean of its leaves' elements, of their decoded values with a
codec, stored as the vector's type; a cell whose leaves all have weight
0 takes their unweighted mean, since a vector is never missing. A mean
vector takes no part in choosing representatives. A file stores a vector
as one fixed-length binary column, so a 64-element vector costs one
column's metadata a tile rather than 64, and one byte an element. Files
with a vector column are format version 3. `a5_pyramid()` and
[`a5_read_pyramid()`](https://belian-earth.github.io/a5pyramid/reference/a5_read_pyramid.md)
return it as an integer matrix column. Vector columns come after the
scalar columns, whatever the order they are selected in. When a vector
is the only `"rep_child"` input, its standard deviation scales every
term alike and cannot change the representative.

## Column types

Every level is stored with one schema, so each value column has one
storage type, set by `types`: `"int8"`, `"int16"`, `"int32"`, `"uint8"`,
`"uint16"`, `"uint32"`, `"float32"` or `"float64"`. Integer columns
default to `"int32"` and double columns to `"float64"`; a column of
Arrow data defaults to its own type (`"float32"` for `float`). Values,
leaves included, are stored as their type: an integer type rounds half
to even (as [`round()`](https://rdrr.io/r/base/Round.html) does) and
clamps to its range, so the mean of a `"uint8"` colour channel is a
valid channel, and `"float32"` rounds to single precision.
`a5_pyramid()` returns the stored values. For a browser, `"uint8"` suits
colour channels and `"float32"` halves the size of continuous values.

## Codecs

A *codec* says how a column's stored codes map to the values they stand
for. With one, a `"mean"` decodes each leaf's code, takes the (weighted)
mean of the values and encodes it, so a coarse cell holds the code of
the mean of its leaves' values rather than the mean of their codes,
which differ for a nonlinear code. The leaves keep their codes as given.
The codec is recorded in the index
([`a5_pyramid_info()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid_info.md))
for readers to decode with. One codec is defined:

- `"aef"`: AlphaEarth Foundations embeddings, `"int8"` codes decoded as
  `sign(x) * (x / 127.5)^2` and encoded as
  `round(sign(v) * sqrt(abs(v)) * 127.5)`, rounded half away from zero
  and clamped to -128 to 127, as the a5embeddings store encodes its cell
  means. A coarse cell holds the code the store would write for the mean
  of its leaves' decoded embeddings; with each leaf's pixel count as
  `weights`, for the mean of their pixels, up to the leaves' own
  rounding. The mean is taken on `x * abs(x)`, the value times 127.5^2,
  which is exact for whole codes and weights, so a mean exactly half way
  between two codes rounds away from zero whatever the order of the sum.

A codec applies to `"mean"` columns, scalar or vector, stored as its
code type.

## See also

[`a5_write_pyramid()`](https://belian-earth.github.io/a5pyramid/reference/a5_write_pyramid.md)
to write the pyramid for a browser,
[`a5_pyramid_arrow()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid_arrow.md)
for the same tiles as an Arrow table.

## Examples

``` r
library(a5R)
#> a5R 1.0.0.9000 • a5 crate 1.0.0 • A5 index 1.0 • 1 thread
centre <- a5_lonlat_to_cell(-3.19, 55.95, resolution = 10)
cells <- a5_uncompact(a5_spherical_cap(centre, 3000), resolution = 15)
x <- data.frame(cell = cells, value = seq_along(cells))

p <- a5_pyramid(x, lods = c(9, 11, 13))
p
#> # A tibble: 1,110 × 4
#>    cell               lod ncell value
#>    <a5_cell>        <int> <int> <int>
#>  1 6316020000000000     9   441   777
#>  2 63bb920000000000     9    88   192
#>  3 63bb960000000000     9   495   334
#>  4 63bb93a000000000    11    64   701
#>  5 63bb942000000000    11   187   135
#>  6 63bb946000000000    11   187   378
#>  7 63bb94a000000000    11   261   591
#>  8 63bb94e000000000    11   192   819
#>  9 63bb952000000000    11    64   957
#> 10 63bb96e000000000    11    69   164
#> # ℹ 1,100 more rows
table(p$lod)
#> 
#>    9   11   13   15 
#>    3    7   76 1024 

# a colour composite: coarse cells keep a real colour, not a blend
x$red <- sample(0:255, nrow(x), replace = TRUE)
x$green <- sample(0:255, nrow(x), replace = TRUE)
a5_pyramid(
  x, red, green,
  aggregate = "rep_child",
  types = c(red = "uint8", green = "uint8")
)
#> # A tibble: 1,426 × 5
#>    cell               lod ncell   red green
#>    <a5_cell>        <int> <int> <int> <int>
#>  1 0c80000000000000     2  1024   126   122
#>  2 6320000000000000     3  1024   126   122
#>  3 6318000000000000     4  1024   126   122
#>  4 6316000000000000     5  1024   126   122
#>  5 63bb800000000000     6  1002   126   122
#>  6 63be800000000000     6    22   123   190
#>  7 6316200000000000     7   467   128   131
#>  8 63bba00000000000     7   557   126   122
#>  9 6316080000000000     8   467   128   131
#> 10 63bb980000000000     8   557   126   122
#> # ℹ 1,416 more rows
```
