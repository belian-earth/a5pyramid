# Write one level of precomputed rows as a pyramid file

Writes rows that were reduced elsewhere, one level of them, as an
a5pyramid file: tiled, boxed and indexed as
[`a5_write_pyramid()`](https://belian-earth.github.io/a5pyramid/reference/a5_write_pyramid.md)
writes its levels, but without building anything. Use it where each
level is its own file and its rows come from somewhere else, such as a
partition of a store whose levels are built from the partition's own
leaves, or coarse levels summed from such partitions' partial sums. Such
files are in format version 4.

## Usage

``` r
a5_write_pyramid_level(
  x,
  dest,
  ...,
  data_resolution = NULL,
  ncell = NULL,
  weights = NULL,
  weights_type = "uint32",
  reducers = NULL,
  types = NULL,
  codecs = NULL,
  rep_sd = NULL,
  storage = NULL,
  shard = NULL,
  owner = FALSE,
  lod_column = FALSE,
  tile_size = 16384L,
  row_group_size = 131072L,
  compression = c("zstd", "snappy", "none"),
  metadata = NULL,
  threads = NULL
)
```

## Arguments

- x:

  A data frame (an
  [a5R::a5_cell](https://belian-earth.github.io/a5R/reference/a5_cell.html)
  column, value columns and integer matrix columns for vectors) or Arrow
  data (a `uint64` cell column, fixed-size or plain lists of `int8` or
  `uint8` for vectors, as DuckDB's `TINYINT[]`) holding one level's
  rows, each cell once, at one resolution. Row order does not matter.

- dest:

  Path of the Parquet file to write. An existing file is replaced. The
  file is written under a temporary name and renamed when complete, so a
  failed or interrupted write leaves no partial file.

- ...:

  Value columns to write, as in
  [`a5_pyramid()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid.md).
  Default: every numeric column but `ncell` and the weights.

- data_resolution:

  The pyramid's leaves' resolution, at least the rows'. Default: the
  rows', for a file of leaves.

- ncell:

  The name of the column holding the leaves each row summarises, needed
  for rows coarser than the leaves. `NULL` (default) for leaves, whose
  file then has no `ncell` column.

- weights:

  The name of the column holding each row's summed leaf weights, stored
  as the weights column (see `weights_type`), or `NULL`.

- weights_type:

  The weights column's type. Default: `"uint32"`.

- reducers, codecs, rep_sd:

  How the rows were reduced, recorded in the index for readers, as in
  [`a5_pyramid()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid.md):
  not applied.

- types:

  Named character vector of storage types for single columns, such as
  `c(red = "uint8", ndvi = "float32")`. See "Column types".

- storage:

  Named character vector of how to store vector columns:
  `"fixed_binary"` (default), one fixed-length binary value a row, the
  smallest footer and the fastest decode; or `"list"`, a Parquet list of
  the elements, dictionary encoded, which engines such as DuckDB read as
  an array column (`TINYINT[]`), as the a5embeddings store keeps its
  embeddings (format version 4). Such as `c(embedding = "list")`.

- shard:

  `NULL`, or a list of the shard's `tier` (from 0), `cell` (an
  [a5R::a5_cell](https://belian-earth.github.io/a5R/reference/a5_cell.html)
  or 16 hex digits) and `resolution`.

- owner:

  Whether to add the `owner` column (needs `shard`).

- lod_column:

  Whether to store the `lod` column. Default `FALSE`: a file of one
  level needs none, the index gives the level.

- tile_size, row_group_size:

  As in
  [`a5_write_pyramid()`](https://belian-earth.github.io/a5pyramid/reference/a5_write_pyramid.md).
  The defaults are the embeddings store's: tiles of 16,384 rows in row
  groups of up to 131,072.

- compression:

  `"zstd"` (default), `"snappy"` or `"none"`.

- metadata:

  Named character vector or list of strings to add to the file's
  key-value metadata, such as a renderer's settings as JSON. The keys
  `"a5pyramid"`, `"a5_index"` and `"ARROW:schema"` are reserved.

- threads:

  Number of threads, or `NULL` (default) for
  [`a5pyramid_threads()`](https://belian-earth.github.io/a5pyramid/reference/a5pyramid_threads.md).

## Value

`dest`, invisibly.

## Shards and partial rows

With `shard`, the file is a shard of a set: the index records its tier
and shard cell, and
[`a5_write_pyramid_set_table()`](https://belian-earth.github.io/a5pyramid/reference/a5_write_pyramid_set_table.md)
can list it. With `owner = TRUE` the file also has an `owner` column,
each row cell's supercell at the shard resolution
([`a5R::a5_cell_to_supercell()`](https://belian-earth.github.io/a5R/reference/a5_spatial_hierarchy.html)).
The rows of such a shard may be *partial*: a cell split by the shard's
edge has a row in each shard holding some of its leaves, each
summarising those leaves alone, and `owner` names the shard containing
the cell's centre. A renderer draws every row; a reader that wants whole
cells merges a cell's rows, weighted by the weights column (or `ncell`),
with the column's codec.

## See also

[`a5_write_pyramid_set_table()`](https://belian-earth.github.io/a5pyramid/reference/a5_write_pyramid_set_table.md)
to list such files as a set.

## Examples

``` r
library(a5R)
centre <- a5_lonlat_to_cell(-3.19, 55.95, resolution = 10)
cells <- a5_uncompact(a5_spherical_cap(centre, 3000), resolution = 15)
leaves <- data.frame(cell = cells, value = runif(length(cells)), n = 4L)

# a level built elsewhere, here by a5_pyramid(), written as a file
p <- a5_pyramid(leaves, value, lods = 12, weights = "n", weights_type = "uint32")
l12 <- p[p$lod == 12, ]
path <- tempfile(fileext = ".parquet")
a5_write_pyramid_level(l12, path, value, data_resolution = 15, ncell = "ncell", weights = "n",
                       tile_size = 100, compression = "snappy")
a5_pyramid_info(path)
#> ── a5pyramid file file29344c4155c5.parquet ─────────────────────────────────────
#> Size: 1.2 KB (footer 739 B)
#> A5 index: 1.0
#> Leaves: resolution 15, 22 cells
#> LODs: 12
#> Rows: 22
#> Tiles: 1 row groups (tile size 100)
#> Columns: value <float64, mean>
#> Bbox: -3.2269, 55.9282, -3.1195, 56.0411
#> Written by: a5pyramid 0.0.0.9000, a5 crate 1.0.0
```
