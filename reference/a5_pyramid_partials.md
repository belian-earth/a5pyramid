# Partial rows of a pyramid's mean vectors

Reduces leaves to exact partial sums at coarse levels, so that pyramids
of separate parts of a region, each built alone (a store's partitions),
can have coarse levels that span the parts.
[`a5_pyramid_merge_partials()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid_merge_partials.md)
adds the partial rows of every part and encodes the means: the rows that
[`a5_pyramid()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid.md)
gives for all the leaves at once.

## Usage

``` r
a5_pyramid_partials(
  x,
  ...,
  lods,
  key_resolution,
  weights = NULL,
  weights_type = "uint32",
  types = NULL,
  codecs = NULL,
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

  The vector columns to sum (tidyselect, as in
  [`a5_pyramid()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid.md)).
  Each is reduced by its mean; other columns do not merge and are
  refused.

- lods:

  The coarse LODs, A5 resolutions below the leaves'.

- key_resolution:

  The resolution of each row's key, the supercell of its cell there: the
  shard its merged row belongs to. One for every LOD, or one for all,
  each no finer than its LOD.

- weights:

  The leaf weights column, every weight above zero (a mean over leaves
  that all weigh nothing has no partial form), or `NULL` for a weight of
  1 each.

- weights_type:

  The type the merged rows store their summed weights in, as
  `weights_type` in
  [`a5_pyramid()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid.md);
  ignored without `weights`.

- types:

  Named character vector of storage types for single columns, such as
  `c(red = "uint8", ndvi = "float32")`. See "Column types".

- codecs:

  Named character vector of codecs for `"mean"` columns whose values are
  codes, such as `c(embedding = "aef")`. See "Codecs". Default `NULL`:
  every column stores its values.

- threads:

  Number of threads, or `NULL` (default) for
  [`a5pyramid_threads()`](https://belian-earth.github.io/a5pyramid/reference/a5pyramid_threads.md).

## Value

An Arrow `Table` in ascending LOD and cell order, with the columns `lod`
(`uint8`), `key` and `cell` (`uint64`), `ncell` (`uint64`), the summed
weights named as `weights` (`float64`) with weights, and each vector's
sums named as the vector (a fixed-size list of `float64`). Its schema
metadata (`"a5pyramid_partials"`) records the vectors' types, sizes and
codecs and the weights' name and type, which
[`a5_pyramid_merge_partials()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid_merge_partials.md)
needs:
[`arrow::write_parquet()`](https://arrow.apache.org/docs/r/reference/write_parquet.html)
keeps it.

## Details

A row holds a cell of a LOD that has a leaf (the cell containing the
leaf's centre,
[`a5R::a5_cell_to_supercell()`](https://belian-earth.github.io/a5R/reference/a5_spatial_hierarchy.html))
and the sums over its leaves: their number, their weights, and for each
vector the weighted sum of every element's value. With a codec the
values are lifted as the builder lifts them for a mean (for `"aef"`,
`code * |code|`), so with integer weights the sums are integers, exact
in doubles below 2^53, and the merged means do not depend on how the
leaves were split.

## See also

[`a5_pyramid_merge_partials()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid_merge_partials.md)
to merge them,
[`a5_write_pyramid_level()`](https://belian-earth.github.io/a5pyramid/reference/a5_write_pyramid_level.md)
to write the merged rows.

## Examples

``` r
library(a5R)
centre <- a5_lonlat_to_cell(-3.19, 55.95, resolution = 9)
cells <- a5_uncompact(a5_spherical_cap(centre, 4000), resolution = 14)
emb <- matrix(sample(-128:127, length(cells) * 4, replace = TRUE), ncol = 4)
x <- tibble::tibble(cell = cells, embedding = emb, n = 1 + seq_along(cells) %% 5)

# two parts, built one at a time
west <- a5_cell_to_supercell(cells, 11) < a5_cell_to_supercell(cells[1], 11)
parts <- lapply(split(seq_along(cells), west), function(i) {
  a5_pyramid_partials(x[i, ], embedding, lods = 10:12, key_resolution = 8,
                      weights = "n", codecs = c(embedding = "aef"))
})
merged <- a5_pyramid_merge_partials(do.call(arrow::concat_tables, unname(parts)))
merged
#> Table
#> 105 rows x 6 columns
#> $lod <uint8 not null>
#> $key <uint64 not null>
#> $cell <uint64 not null>
#> $ncell <uint32 not null>
#> $n <uint32 not null>
#> $embedding: fixed_size_list<element <int8 not null>[4] not null>
#> 
#> See $metadata for additional Schema metadata
```
