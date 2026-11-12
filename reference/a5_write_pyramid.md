# Write a level-of-detail pyramid of A5 cells to Parquet

Builds the pyramid of
[`a5_pyramid()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid.md)
and writes it, leaves included, to one Parquet file laid out for a
browser to read with HTTP range requests, as with hyparquet. The build
and the write run in Rust, so the levels never pass through R.

## Usage

``` r
a5_write_pyramid(
  x,
  dest,
  ...,
  lods = NULL,
  aggregate = c("mean", "rep_child"),
  reducers = NULL,
  weights = NULL,
  types = NULL,
  rep_sd = NULL,
  codecs = NULL,
  tile_size = 4096L,
  row_group_size = NULL,
  storage = NULL,
  weights_type = NULL,
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

  Path of the Parquet file to write. An existing file is replaced. The
  file is written under a temporary name and renamed when complete, so a
  failed or interrupted write leaves no partial file.

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

- tile_size:

  The most rows in a tile. Larger tiles mean fewer requests and a
  smaller footer; smaller tiles mean less data fetched outside the view.
  Default: 4096.

- row_group_size:

  `NULL` (default) for one tile a row group, or the most rows in a row
  group of several tiles, at least `tile_size` (see "File layout"). The
  embeddings store uses 131072 with tiles of 16384.

- storage:

  Named character vector of how to store vector columns:
  `"fixed_binary"` (default), one fixed-length binary value a row, the
  smallest footer and the fastest decode; or `"list"`, a Parquet list of
  the elements, dictionary encoded, which engines such as DuckDB read as
  an array column (`TINYINT[]`), as the a5embeddings store keeps its
  embeddings (format version 4). Such as `c(embedding = "list")`.

- weights_type:

  `NULL` (default), or a storage type (see "Column types") in which to
  keep each row's summed `weights` at every level, as a column named as
  the weights, beside the value columns and not one of them: a reader
  that merges rows of one cell weights them by it (format version 4 in a
  file). Needs `weights`.

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

## File layout

Each level is split into tiles of at most `tile_size` rows, and each
tile is one Parquet row group. Levels follow each other in ascending LOD
order, coarsest first. A tile is a run of consecutive cell ids: A5 ids
follow a space-filling curve, so such a run is a compact patch of the
globe. A level is split down the A5 index hierarchy only where a run
holds more than `tile_size` cells, which makes tiles coarse where data
is sparse and fine where it is dense, and keeps every level sorted by
cell id.

With `row_group_size`, each level is split down the hierarchy into row
groups of at most `row_group_size` rows instead, and each row group into
tiles of `tile_size` rows (the last shorter), each one data page of
every column. Engines that read whole row groups, such as DuckDB over
S3, then make fewer requests, while a browser still reads one tile at a
time: the file carries the Parquet page index, which locates each tile's
pages. Such a file is in format version 4.

The file's key-value metadata holds a JSON index under `"a5pyramid"`:
the leaves' resolution, the value columns with their types and reducers,
each level's LOD, row count and tiles, and the bounding box of each
tile, computed from its cells' boundaries. A reader fetches the footer,
picks the level for the zoom and the tiles whose boxes meet the view,
and decodes only those row groups. `"a5_index"` records the A5 index
version of the ids, `"1.0"`, as across the a5R stack.

The columns are `cell` (`uint64`), `lod` (`uint8`), `ncell` (`uint32`)
and the value columns with their types, `NA` stored as null. `cell` and
`ncell` are delta encoded and `lod` dictionary encoded; Parquet
statistics are kept for `cell` and `lod` only, so engines such as DuckDB
can skip row groups by LOD or cell while the footer stays small. The
format is specified in `FORMAT.md` in the package source
(`src/rust/a5pyramid/FORMAT.md`), with a reading recipe for hyparquet.

## See also

[`a5_pyramid_info()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid_info.md)
to inspect the file,
[`a5_read_pyramid()`](https://belian-earth.github.io/a5pyramid/reference/a5_read_pyramid.md)
to read levels back,
[`a5_pyramid_arrow()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid_arrow.md)
for the same tiles in memory.

## Examples

``` r
library(a5R)
centre <- a5_lonlat_to_cell(-3.19, 55.95, resolution = 10)
cells <- a5_uncompact(a5_spherical_cap(centre, 3000), resolution = 15)
x <- data.frame(cell = cells, value = runif(length(cells)))

path <- tempfile(fileext = ".parquet")
a5_write_pyramid(x, path, types = c(value = "float32"), tile_size = 1000)
a5_pyramid_info(path)
#> ── a5pyramid file file293427d11930.parquet ─────────────────────────────────────
#> Size: 13.2 KB (footer 4.8 KB)
#> A5 index: 1.0
#> Leaves: resolution 15, 1,024 cells
#> LODs: 2-15 (14 levels)
#> Rows: 1,426
#> Tiles: 15 row groups (tile size 1000)
#> Columns: value <float32, mean>
#> Bbox: -3.2194, 55.9355, -3.127, 56.0338
#> Written by: a5pyramid 0.0.0.9000, a5 crate 1.0.0
```
