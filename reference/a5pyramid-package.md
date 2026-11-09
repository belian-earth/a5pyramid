# a5pyramid: Level-of-Detail Pyramids of A5 Cells for Browser Rendering

a5pyramid builds level-of-detail pyramids of [A5](https://a5geo.org/)
cells, as created by
[a5R](https://belian-earth.github.io/a5R/reference/a5R-package.html) and
a5px, and writes them with the cells they summarise to one Parquet file
laid out for browsers. A web map reads the file's footer, picks the
level for its zoom, and fetches only the tiles in view with HTTP range
requests, for example with hyparquet and deck.gl. The engine is a Rust
crate with no R dependency, built to be published on its own.

## Building

- [`a5_pyramid()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid.md):
  build the levels in memory and return them as a tibble.

- [`a5_write_pyramid()`](https://belian-earth.github.io/a5pyramid/reference/a5_write_pyramid.md):
  build the levels and write them to Parquet, in Rust from end to end.

- [`a5_pyramid_arrow()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid_arrow.md):
  build the levels and return the tiles of the Parquet layout in memory,
  as an Arrow table, without a file.

Each coarser level groups the leaves by the coarse cell containing their
centres
([`a5R::a5_cell_to_supercell()`](https://belian-earth.github.io/a5R/reference/a5_spatial_hierarchy.html))
and reduces each column by mean, sum, minimum, maximum, mode or a
representative leaf.

## Reading

- [`a5_pyramid_info()`](https://belian-earth.github.io/a5pyramid/reference/a5_pyramid_info.md):
  the file's index and layout, from its footer.

- [`a5_read_pyramid()`](https://belian-earth.github.io/a5pyramid/reference/a5_read_pyramid.md):
  the rows of chosen levels within a bounding box, read as a renderer
  reads them.

## Threads

[`a5pyramid_threads()`](https://belian-earth.github.io/a5pyramid/reference/a5pyramid_threads.md)
sets how many threads builds use; the default is every available CPU.
Builds can be interrupted (Ctrl-C, or Esc in an IDE).

## Errors

Errors raised by a5pyramid have class `a5pyramid_error` and one of:

- `a5pyramid_error_input`: invalid arguments or cells, such as cells at
  mixed resolutions or a duplicated cell.

- `a5pyramid_error_io`: a file or directory that cannot be read or
  written.

- `a5pyramid_error_format`: a file that is not a pyramid, or one written
  for another A5 index or a newer format version.

- `a5pyramid_error_interrupted`: a build stopped by the user.

- `a5pyramid_error_internal`: a fault in a5pyramid; please report it.

## See also

Useful links:

- <https://github.com/belian-earth/a5pyramid>

- <https://belian-earth.github.io/a5pyramid/>

- Report bugs at <https://github.com/belian-earth/a5pyramid/issues>

## Author

**Maintainer**: Hugh Graham <hugh@belian.earth>

Authors:

- Hugh Graham <hugh@belian.earth>

Other contributors:

- belian.earth \[copyright holder\]
