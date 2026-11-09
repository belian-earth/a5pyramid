# Changelog

## a5pyramid (development version)

- First version: a Rust engine, with R bindings, that builds
  level-of-detail pyramids of A5 cells and writes them to one Parquet
  file for browser rendering. It replaces the R implementation in a5view
  0.3.
- [`a5_pyramid()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid.md)
  builds the levels and returns them as a tibble;
  [`a5_write_pyramid()`](https://belian-earth.github.io/a5pyramid/reference/a5_write_pyramid.md)
  builds and writes them to Parquet in Rust. Each coarser level groups
  the leaves by supercell
  ([`a5R::a5_cell_to_supercell()`](https://belian-earth.github.io/a5R/reference/a5_spatial_hierarchy.html)),
  from the leaves directly, and reduces each column by `"mean"`,
  `"sum"`, `"min"`, `"max"`, `"mode"` or `"rep_child"`, optionally
  weighted. `"rep_child"` scales each column by its standard deviation,
  so units do not decide the representative, and breaks ties by lowest
  cell id; a5view used unscaled distances and let rounding break ties.
  Columns take a storage type (`"uint8"` to `"float64"`) shared by every
  level.
- [`a5_pyramid_arrow()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid_arrow.md)
  returns the tiles of a pyramid in memory as an Arrow table, one record
  batch per tile, with the index in the schema metadata: the file’s
  layout without the file. The tiles cross from Rust through the Arrow C
  stream interface, so no value passes through R. Record batch `i` holds
  the rows of row group `i` of the same pyramid written with
  [`a5_write_pyramid()`](https://belian-earth.github.io/a5pyramid/reference/a5_write_pyramid.md).
- [`a5_pyramid_info()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid_info.md)
  reads a file’s index from its footer, with a `tiles` table of every
  tile’s level, rows, bytes and box, and
  [`a5_read_pyramid()`](https://belian-earth.github.io/a5pyramid/reference/a5_read_pyramid.md)
  reads levels within a bounding box, as a renderer does.
- [`a5pyramid_threads()`](https://belian-earth.github.io/a5pyramid/reference/a5pyramid_threads.md)
  sets the session’s thread count, by default every available CPU.
  Results do not depend on it, and builds can be interrupted.
- The file format (`src/rust/a5pyramid/FORMAT.md`) differs from
  a5view’s:
  - tiles are runs of consecutive cell ids split along the A5 hierarchy
    to at most `tile_size` rows, so sparse data no longer makes many
    tiny row groups, and each tile is one row group;
  - row group boxes come from cell boundaries rather than padded
    centroids, and cross the antimeridian (`west > east`) instead of
    widening to the whole globe;
  - the index is one JSON object under the key `"a5pyramid"`, with the
    levels, the value columns, and each tile’s box as flat arrays. It
    names tiles `tiles` and `tile_bbox`, the same in the Parquet file
    and the Arrow form (`row_groups` and `row_group_bbox` before
    2026-11-09, which are still read);
  - columns are `cell` (`uint64`), `lod`, `ncell` and the value columns
    under their own names; `cell` is delta encoded, which takes it from
    8 bytes per row to about 3.
- On 439,276 cells, writing a pyramid takes 0.48 s on 20 threads against
  4.06 s with a5view 0.3, and the file is 28% smaller.
- The engine is the `a5pyramid` Rust crate in `src/rust/a5pyramid`, with
  no R dependency, to be published once the format settles. The R
  bindings are a separate crate in the same workspace. Its
  `TiledPyramid` holds the tiles as Arrow record batches and streams
  them as an arrow-rs `RecordBatchReader`; `write_parquet()` writes them
  as row groups.
