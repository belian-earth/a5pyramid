# a5pyramid (development version)

* a5R comes from CRAN: `Remotes` no longer names `belian-earth/a5R`, now that
  a5R 1.0.0 is released. An unpinned remote is a request for a5R's default
  branch, and pak refused to install a5pyramid beside any other pinned a5R
  ref (belian-production's base image pins a5R by commit).
* The rest of format version 4, for one store that engines and browsers
  read alike (belian-production `docs/store-v4.md`):
  * **List vectors:** `storage = c(embedding = "list")` stores a vector as
    a Parquet list of its elements, dictionary encoded, which DuckDB reads
    as `TINYINT[]`; Arrow list columns (DuckDB's output) are accepted as
    vectors, each row checked for `dim` elements. `columns$storage` in the
    info functions.
  * **A weights column:** `weights_type` keeps each row's summed `weights`
    at every level, named as the weights and not a value column, so a
    reader can merge rows of one cell (`weights` in the info functions).
  * **`a5_write_pyramid_level()`** writes one level of precomputed rows as
    a file, optionally a shard with an `owner` column (each row cell's
    supercell at the shard resolution) for a partition's partial rows,
    without `lod` (one level) and without `ncell` (leaves). Rows are
    sorted and checked, not built. In the crate: `Pyramid::from_rows()`,
    `TileOptions::owner`, `lod_column`, `ncell_column`.
  * **Shard tables:** `a5_write_pyramid_set(index = "parquet")` writes
    `set.parquet`, one row per shard with the set's fields in the footer,
    boxes unwrapped; `a5_write_pyramid_set_table()` writes one for shards
    written apart, listing empty shards and carrying the writer's own
    columns. `a5_pyramid_set_info()` reads both forms. In the crate:
    `write_set_table()`, `read_set_table()`, `SetMeta::assemble()`,
    `SetIndexFormat`; `ShardEntry::path` and `bbox` are optional.
  * FORMAT.md: "Partial rows" (draw every row; merge by weights) and
    "Shard tables".

* Row groups of several tiles (format version 4): `row_group_size` in
  `a5_write_pyramid()` and in a set's `tiers` splits each level down the
  hierarchy into row groups of at most that many rows, and each row group
  into tiles of `tile_size` rows, each one data page of every column chunk.
  The file carries the Parquet page index (an offset index for every
  column, a column index for `cell`), and the writer checks every page
  against the tiles before the file is renamed into place. Engines that
  read whole row groups make fewer requests: the embeddings store's
  prototype replay over S3 ran 27 to 41% slower with 16,384-row groups than
  with 122,880, while a browser reading 16,384-row pages of 131,072-row
  groups fetched the same bytes as with 16,384-row groups. Without
  `row_group_size` files are unchanged. `a5_pyramid_info()` gains
  `row_group_size`, `row_groups` and `tiles$row_group`, and `tiles$bytes`
  counts a tile's pages; data pages are pinned to version 1. In the crate:
  `TileOptions::row_group_size`, `Tier::row_group_size`,
  `PyramidMeta::row_group_size`, `PyramidMeta::locate_tiles()`,
  `TiledPyramid::row_groups()`, `PyramidFile::tiles()`, `tile_bytes()` and
  `parquet_metadata()`; `PyramidFile::read()` takes tiles, and
  `WriteSummary` gains `tiles`.

* Codecs: `codecs = c(embedding = "aef")` gives a `mean` column of codes
  the mean of the values they stand for. Each leaf's code is decoded, the
  (weighted) mean taken and the result encoded, so a coarse cell holds the
  code of the mean rather than the mean of the codes, which differ for a
  nonlinear code. `"aef"` is the AlphaEarth Foundations int8 code, encoded
  as the a5embeddings store encodes its cell means (rounded half away from
  zero, clamped to -128 to 127), taking the mean on `x * |x|` so that whole
  codes and weights sum exactly and a mean half way between two codes
  rounds away from zero in any order. The index records each column's `codec`
  (`columns$codec` in the info functions), an optional field added without
  a format version change. In the crate: `Codec`, `ColumnSpec::codec`,
  `VectorSpec::codec`.
* Vector columns can be reduced by `mean`, element by element, as well as
  `rep_child`: `reducers = c(embedding = "mean")`. Vectors stay
  `rep_child` by default, whatever `aggregate` says, and a `mean` vector
  takes no part in choosing representatives. In the crate:
  `VectorSpec::reducer`, `VectorColumn::with_reducer()`.

* Vector columns (format version 3): a fixed number of `int8` or `uint8`
  values per cell, such as a 64-element embedding, from an integer matrix
  column of a data frame or a fixed-size list (DuckDB's `TINYINT[64]`) in
  Arrow data. The engine holds one byte an element; files store the vector
  as one fixed-length binary column, so its footer and requests grow like
  one column's, not 64. Vectors are reduced by `rep_child` with one standard
  deviation over all their elements (fixed by `rep_sd`), together with any
  scalar `rep_child` columns. `a5_pyramid()` and `a5_read_pyramid()` return
  them as integer matrix columns; `columns$dim` in the info functions. Files
  and sets without vector columns keep versions 1 and 2. In the crate:
  `VectorColumn`, `VectorSpec`, `Leaves::vectors`, `Level::vectors`.

* Pyramid sets: `a5_write_pyramid_set()` writes one pyramid as a directory
  of small files, *shards*, with an index, `set.json`, for data too large
  for one file to open in a browser, whose footer grows by about 365 bytes
  a tile (about 20 MB for 150 million leaves). The LODs are split into
  tiers; a cell belongs to the shard keyed by its supercell at the tier's
  shard resolution, so every cell is in exactly one shard, and levels are
  built from all the leaves before the split. `a5_pyramid_set_tiers()`
  gives the default tiers (three LODs a tier, shards 12 resolutions coarser
  than the tier's finest LOD, so a view meets a few shards at any zoom),
  and `a5_pyramid_set_info()` reads the index. A write that fails or is
  interrupted removes what it wrote, so it can be run again into the same
  directory. In the crate: `write_set()`, `SetMeta`, `Tier` and
  `default_tiers()`.
* Format version 2 (`FORMAT.md`, "Pyramid sets"): shards are a5pyramid
  files whose index records the `shard` (tier, shard cell, resolution),
  whose finest level need not be the leaves, and whose `bbox` covers every
  tile rather than only the leaves. A writer records the oldest version a
  file follows, so single files are still version 1 and a5view reads them
  as before; readers of this version read both. `a5_pyramid_info()` returns
  the `shard` and prints a shard as one.
* Arrow data as input: `x` may be an Arrow `Table`, `RecordBatch`,
  `RecordBatchReader`, `Dataset` or query with one `uint64` cell column.
  It is read into the engine batch by batch through the Arrow C stream
  interface, without passing through R, so a build from
  `arrow::open_dataset()` over Parquet files holds no copy of the leaves in
  R; value columns default to their own Arrow types.
* `rep_sd` fixes the standard deviations that scale `"rep_child"` columns,
  so pyramids of separate parts of a region choose a cell's representative
  from its own leaves alone. The standard deviations used, given or
  computed, are recorded in the index (`sd` in `columns`) and returned by
  `a5_pyramid_info()` and `a5_pyramid_set_info()`.
* Builds need less memory: sorting the leaves rewrites the cells in place
  and gathers one column at a time, levels and shards are grouped without
  holding their row pairs twice, and shard rows are grouped by a counting
  sort (4 bytes a row instead of 16). The Sabah AEF set (149.6 million
  res-18 leaves, three bands) peaks at 13.3 GB from a data frame, down from
  21.2 GB, and at 9.3 GB from an Arrow `Dataset`, with identical shards.
* First version: a Rust engine, with R bindings, that builds
  level-of-detail pyramids of A5 cells and writes them to one Parquet file
  for browser rendering. It replaces the R implementation in a5view 0.3.
* `a5_pyramid()` builds the levels and returns them as a tibble;
  `a5_write_pyramid()` builds and writes them to Parquet in Rust.
  Each coarser level groups the leaves by supercell
  (`a5R::a5_cell_to_supercell()`), from the leaves directly, and reduces
  each column by `"mean"`, `"sum"`, `"min"`, `"max"`, `"mode"` or
  `"rep_child"`, optionally weighted. `"rep_child"` scales each column by
  its standard deviation, so units do not decide the representative, and
  breaks ties by lowest cell id; a5view used unscaled distances and let
  rounding break ties. Columns take a storage type (`"uint8"` to
  `"float64"`) shared by every level.
* `a5_pyramid_arrow()` returns the tiles of a pyramid in memory as an
  Arrow table, one record batch per tile, with the index in the schema
  metadata: the file's layout without the file. The tiles cross from Rust
  through the Arrow C stream interface, so no value passes through R.
  Record batch `i` holds the rows of row group `i` of the same pyramid
  written with `a5_write_pyramid()`.
* `a5_pyramid_info()` reads a file's index from its footer, with a
  `tiles` table of every tile's level, rows, bytes and box, and
  `a5_read_pyramid()` reads levels within a bounding box, as a renderer
  does.
* `a5pyramid_threads()` sets the session's thread count, by default every
  available CPU. Results do not depend on it, and builds can be
  interrupted.
* The file format (`src/rust/a5pyramid/FORMAT.md`) differs from a5view's:
  - tiles are runs of consecutive cell ids split along the A5 hierarchy
    to at most `tile_size` rows, so sparse data no longer makes many tiny
    row groups, and each tile is one row group;
  - row group boxes come from cell boundaries rather than padded centroids,
    and cross the antimeridian (`west > east`) instead of widening to the
    whole globe;
  - the index is one JSON object under the key `"a5pyramid"`, with the
    levels, the value columns, and each tile's box as flat arrays. It names
    tiles `tiles` and `tile_bbox`, the same in the Parquet file and the
    Arrow form (`row_groups` and `row_group_bbox` before 2026-11-09, which
    are still read);
  - columns are `cell` (`uint64`), `lod`, `ncell` and the value columns
    under their own names; `cell` is delta encoded, which takes it from 8
    bytes per row to about 3.
* On 439,276 cells, writing a pyramid takes 0.48 s on 20 threads against
  4.06 s with a5view 0.3, and the file is 28% smaller.
* The engine is the `a5pyramid` Rust crate in `src/rust/a5pyramid`, with no
  R dependency, to be published once the format settles. The R bindings
  are a separate crate in the same workspace. Its `TiledPyramid` holds the
  tiles as Arrow record batches and streams them as an arrow-rs
  `RecordBatchReader`; `write_parquet()` writes them as row groups.
