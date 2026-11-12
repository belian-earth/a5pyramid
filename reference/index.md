# Package index

## Building pyramids

Aggregate A5 cells to coarser levels of detail, as a tibble, an Arrow
table or a Parquet file.

- [`a5_pyramid()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid.md)
  : Build a level-of-detail pyramid of A5 cells
- [`a5_pyramid_arrow()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid_arrow.md)
  : Build a level-of-detail pyramid as an Arrow table
- [`a5_write_pyramid()`](https://belian-earth.github.io/a5pyramid/reference/a5_write_pyramid.md)
  : Write a level-of-detail pyramid of A5 cells to Parquet
- [`a5_write_pyramid_level()`](https://belian-earth.github.io/a5pyramid/reference/a5_write_pyramid_level.md)
  : Write one level of precomputed rows as a pyramid file

## Reading pyramids

A file’s index and levels, read as a renderer reads them.

- [`a5_pyramid_info()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid_info.md)
  : Inspect a pyramid file
- [`a5_read_pyramid()`](https://belian-earth.github.io/a5pyramid/reference/a5_read_pyramid.md)
  : Read levels of a pyramid file

## Pyramid sets

One pyramid as many small files, so a browser opens only those around a
view.

- [`a5_write_pyramid_set()`](https://belian-earth.github.io/a5pyramid/reference/a5_write_pyramid_set.md)
  : Write a pyramid as a set of shard files
- [`a5_pyramid_set_tiers()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid_set_tiers.md)
  : Default tiers of a pyramid set
- [`a5_pyramid_set_info()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid_set_info.md)
  : Inspect a pyramid set
- [`a5_write_pyramid_set_table()`](https://belian-earth.github.io/a5pyramid/reference/a5_write_pyramid_set_table.md)
  : Write a set's shard table

## Partial rows

Exact sums at coarse levels for parts built one at a time, merged into
levels that span them.

- [`a5_pyramid_partials()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid_partials.md)
  : Partial rows of a pyramid's mean vectors
- [`a5_pyramid_merge_partials()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid_merge_partials.md)
  : Merge partial rows into a pyramid's coarse levels

## Configuration

- [`a5pyramid_threads()`](https://belian-earth.github.io/a5pyramid/reference/a5pyramid_threads.md)
  [`with(`*`<a5pyramid_threads>`*`)`](https://belian-earth.github.io/a5pyramid/reference/a5pyramid_threads.md)
  : Get or set the number of threads a5pyramid uses

## Package overview

- [`a5pyramid`](https://belian-earth.github.io/a5pyramid/reference/a5pyramid-package.md)
  [`a5pyramid-package`](https://belian-earth.github.io/a5pyramid/reference/a5pyramid-package.md)
  : a5pyramid: Level-of-Detail Pyramids of A5 Cells for Browser
  Rendering
