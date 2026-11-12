# Merge partial rows into a pyramid's coarse levels

Adds the partial rows
([`a5_pyramid_partials()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid_partials.md))
of every part of a region, cell by cell, and encodes each mean as
[`a5_pyramid()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid.md)
does: the code of the mean of the lifted values. The rows of one LOD and
key are one shard of the coarse levels, ready for
[`a5_write_pyramid_level()`](https://belian-earth.github.io/a5pyramid/reference/a5_write_pyramid_level.md)
(with `ncell = "ncell"` and the weights).

## Usage

``` r
a5_pyramid_merge_partials(x, keys = NULL, threads = NULL)
```

## Arguments

- x:

  Partial rows: an Arrow `Table`, `RecordBatchReader` or `Dataset` (such
  as
  [`arrow::open_dataset()`](https://arrow.apache.org/docs/r/reference/open_dataset.html)
  over the parts' Parquet files), with the schema metadata
  [`a5_pyramid_partials()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid_partials.md)
  writes. Batches are read one at a time, and only the rows of `keys`
  are kept.

- keys:

  Optional
  [a5R::a5_cell](https://belian-earth.github.io/a5R/reference/a5_cell.html)
  vector (or hex strings): merge only the rows of these keys, such as
  one shard's.

- threads:

  Number of threads, or `NULL` (default) for
  [`a5pyramid_threads()`](https://belian-earth.github.io/a5pyramid/reference/a5pyramid_threads.md).

## Value

An Arrow `Table` in ascending LOD, key and cell order: `lod` (`uint8`),
`key` and `cell` (`uint64`), `ncell` (`uint32`), the summed weights in
their stored type (named as the weights), and each vector's codes, a
fixed-size list of its element type.

## See also

[`a5_pyramid_partials()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid_partials.md).
