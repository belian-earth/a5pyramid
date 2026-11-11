# The a5pyramid file format, version 3

An a5pyramid file is one Parquet file holding [A5](https://a5geo.org/) cells
at one resolution (the *leaves*, with their values) and coarser *levels of
detail* (LODs) aggregated from them. It is laid out so that a client, such as
a web map reading it with [hyparquet](https://github.com/hyparam/hyparquet),
fetches the footer and then only the row groups a view needs, with HTTP range
requests.

A pyramid too large for one file is stored as a *pyramid set*: a directory
of a5pyramid files, *shards*, each holding some LODs of the cells around one
coarse cell, and an index, `set.json` (see "Pyramid sets"). Version 2 adds
sets; a single file is unchanged from version 1. Version 3 adds vector
columns (see "Vector columns"), such as embeddings.

The same layout also exists in memory as Arrow record batches (see "The
Arrow form"), for clients that take a pyramid without a file.

This document is the contract between writers (the `a5pyramid` crate and the
R package built on it) and readers (renderers). Key words follow RFC 2119.

## Versions

A writer records in `format_version` the oldest version whose rules a file
follows: 1 for a single file, 2 for a shard of a set, and 3 for either when
it has a vector column (and for the `set.json` of a set whose shards have
one). A reader that supports a version reads every older one. A version-1
reader refuses shards, as it must, because a shard breaks two version-1
rules: its finest level need not be the leaves, and its levels hold only the
cells it owns. A version-2 reader refuses files with vector columns, whose
type it does not know.

## Reading a file

1. Fetch the Parquet footer. Read the key-value metadata entries `a5pyramid`
   (the index, JSON) and `a5_index`.
2. Refuse the file if `format_version` in the index is newer than the reader
   supports, or if `a5_index` is not the A5 index version the reader's A5
   library uses (`"1.0"` for a5-js 1.x). Ids of different index versions look
   alike, and nothing in an id records its version.
3. Pick a level for the view: typically the finest `lod` at or below the A5
   resolution that suits the zoom, else the coarsest level.
4. Take that level's tiles (`levels[i].tiles`, a half-open range) and keep
   those whose box in `tile_bbox` meets the viewport.
5. Fetch and decode those tiles' row groups. Draw each `cell` from its
   boundary (a5-js `cellToBoundary`), coloured from the value columns.

A reader never needs to decode a tile to decide whether to fetch it.

## Columns

| Column | Parquet type | Arrow type | Content |
|---|---|---|---|
| `cell` | `INT64`, `INTEGER(64, false)` | `uint64` | A5 cell id. Never null. |
| `lod` | `INT32`, `INTEGER(8, false)` | `uint8` | The cell's resolution, the row's level. Never null. |
| `ncell` | `INT32`, `INTEGER(32, false)` | `uint32` | The number of leaves the row summarises; 1 at the leaves. Never null. |
| value columns | see below | see below | One per column in the index's `columns`, in that order: the scalar columns, then the vector columns. Null where missing (vectors are never missing). |

Value column types (`type` in the index) are `int8`, `int16`, `int32`,
`uint8`, `uint16`, `uint32`, `float32` and `float64`, stored as the matching
Parquet `INT32`/`FLOAT`/`DOUBLE` physical type with an `INTEGER` logical type
for the integer types. hyparquet decodes `cell` as `BigInt` and every other
column as `Number`.

The names `cell`, `lod` and `ncell` are reserved: no value column takes them.

### Vector columns

A vector column (version 3) holds `dim` values of an 8-bit type per row,
such as an embedding. Its index entry has `dim` and a `type` of `int8` or
`uint8`. It is stored as Parquet `FIXED_LEN_BYTE_ARRAY` of length `dim`
(Arrow `fixed_size_binary(dim)`), plain encoded, without statistics: one
column chunk a tile, where `dim` scalar columns would take `dim` chunks and
grow the footer by `dim` times as much. Byte `e` of a row is element `e`, as
its two's complement for `int8`. hyparquet returns each row as a
`Uint8Array` of `dim` bytes; read `int8` elements through an `Int8Array`
view of the same bytes. A vector is never null.

A vector column is reduced by `rep_child` or by `mean` (see "Levels").

A `mean` vector is reduced element by element: each element of a coarse
cell is the (weighted) mean of its leaves' elements, of their decoded values
where the column has a `codec` (see "Codecs"), stored as the column's type.
A vector is never null, so a cell whose leaves all have weight 0 takes their
unweighted mean. A `mean` vector takes no part in choosing representatives.

In a `rep_child` vector every element takes part in the distance to the
cell's mean, with one `sd` for all its elements. A writer computes that
`sd` over every element of every leaf, pooled: the sample standard
deviation (n − 1) about the elements' grand mean, or 1 where it is 0 or
undefined. When the vector is a pyramid's only
`rep_child` input, its `sd` scales every term of the distance alike and
cannot change a cell's representative, so builds of separate parts of a
region need not fix it (cells at the seams still differ, as their leaves
do); with scalar `rep_child` columns beside it, fix the `sd`s as for any
`rep_child` column.

## Tiles

A *tile* is the unit a reader fetches: one row group of the Parquet file, one
record batch of the Arrow form. Tiles are numbered in order from 0, so tile
`i` is row group `i` and record batch `i`.

- Every tile holds rows of one level.
- Levels follow each other in ascending `lod` order, the leaves last.
- Within a level, rows are in ascending `cell` order, in tiles and across
  them, so every level is sorted by cell id.
- A tile is a run of consecutive cell ids, at most `tile_size` rows. A5 ids
  follow a space-filling curve, so a run of ids is a compact patch of the
  globe. A level is split down the A5 index hierarchy only where a run holds
  more than `tile_size` cells, and neighbouring small tiles under one parent
  are merged, so tiles are coarse where data is sparse and fine where it is
  dense. Tiles never span two resolution-0 cells.

Readers SHOULD NOT depend on how tiles are chosen beyond these properties.

Parquet chunk statistics (min and max) are written for `cell` and `lod`, and
for no other column. A reader can find the tile holding a given cell by
binary search over the `cell` statistics of its level's row groups.

## Key-value metadata

| Key | Value |
|---|---|
| `a5pyramid` | The index, a JSON object (below). |
| `a5_index` | The A5 index version of the cell ids: `"1.0"`. The same key is used across the a5R, a5px and a5view packages. |

Other keys hold application metadata, such as a renderer's settings, and are
opaque to this format. No `ARROW:schema` entry is written: Parquet's logical
types give every column's Arrow type, and Arrow readers then expose the
key-value metadata as schema metadata.

## The index

The index of a small file (1,024 leaves at resolution 15, levels at 11 and
13, `tile_size` 600, so the leaves take two tiles):

```json
{
  "format_version": 1,
  "generator": "a5pyramid 0.0.0.9000",
  "a5_crate": "1.0.0",
  "data_resolution": 15,
  "tile_size": 600,
  "bbox": [-3.21943, 55.93546, -3.12697, 56.03382],
  "columns": [{"name": "value", "type": "float32", "reducer": "mean"}],
  "levels": [
    {"lod": 11, "rows": 7, "tiles": [0, 1]},
    {"lod": 13, "rows": 76, "tiles": [1, 2]},
    {"lod": 15, "rows": 1024, "tiles": [2, 4]}
  ],
  "tile_bbox": [
    -3.23431, 55.92096, -3.12361, 56.03933,
    -3.22314, 55.93183, -3.12324, 56.03745,
    -3.19755, 55.93546, -3.12697, 55.99746,
    -3.21942, 55.97182, -3.14877, 56.03382
  ]
}
```

| Field | Meaning |
|---|---|
| `format_version` | The oldest version of this document whose rules the file follows (see "Versions"). Readers MUST refuse newer versions. Fields added without a version change are optional, and readers MUST ignore fields they do not know. |
| `generator` | The software that wrote the file. |
| `a5_crate` | The version of the a5 Rust crate that computed the levels and boxes. |
| `data_resolution` | The leaves' resolution. In a single file it is the finest level's `lod`; a shard of a tier coarser than the leaves holds no level at it. |
| `tile_size` | The most rows in a tile. |
| `bbox` | `[west, south, east, north]` (see "Boxes"): in a single file, of the leaves; in a shard, of every tile, since coarse cells reach past the leaves under them. |
| `columns` | The value columns in column order: `name`, storage `type` and `reducer` (see "Levels"), and for a `rep_child` column `sd`, the standard deviation that scaled it (1 where the leaves' standard deviation is 0 or undefined, as for a constant column, so `sd` is the scale used rather than a statistic of the data). `sd` is optional (absent before 2026-11-09), and readers that do not choose representatives ignore it. A `mean` column of codes has `codec`, how its codes map to values (see "Codecs"); the field is optional, added without a version change (absent before 2026-11-11 and for columns that store their values), and a reader that ignores it reads the codes. A vector column (version 3) also has `dim`, its elements per row. |
| `levels` | One entry per level, ascending `lod`: its `lod`, its number of `rows`, and the half-open range `[start, end)` of its tiles. Consecutive levels' ranges meet, and together cover every tile. |
| `tile_bbox` | The box of every tile in order, flat: tile `i` is at indices `4i` to `4i + 3`. |
| `shard` | Shards only (version 2): `{"tier", "cell", "resolution"}`, the shard's tier, its shard cell as 16 hex digits, and that cell's resolution. Absent in a single file. |

Writers before 2026-11-09 named `tiles` `row_groups` and `tile_bbox`
`row_group_bbox`, with the same meaning. Readers MAY accept those names.

## Boxes

A box is `[west, south, east, north]` in WGS 84 degrees. As in GeoJSON
(RFC 7946, section 5.2), `west > east` means the box crosses the antimeridian;
a box containing a pole spans every longitude (`-180` to `180`) and reaches
the pole. A tile's box contains every vertex of every cell's boundary in
it, as a5 `cell_to_boundary` gives it with its default edge segments, which is
what a5-js `cellToBoundary` draws. Boxes are rounded outward to 5 decimal
places (about 1 m).

To test a box against a viewport, split each into one or two longitude
intervals within -180 to 180 and compare every pair, as well as the latitude
ranges.

## Levels

A cell at LOD `r` summarises the leaves whose centres it contains
(a5 `cell_to_supercell`), so every level partitions the leaves and `ncell`
sums to the number of leaves in every level. Each level is grouped from the
leaves directly: supercells do not chain, so a level aggregated from the level
below would place some leaves in the wrong coarse cell. In a set the same
holds of a level taken over all the shards of its tier; one shard holds only
the cells it owns.

Each value column is reduced over the leaves under each cell by its
`reducer`:

| Reducer | Value of a coarse cell |
|---|---|
| `mean` | The mean, weighted by the leaves' weights when the writer had them. With a `codec`, the code of the mean of the leaves' decoded values. |
| `sum`, `min`, `max` | The sum, minimum or maximum. |
| `mode` | The most frequent value (by total weight), ties to the smallest. |
| `rep_child` | The value of one *representative* leaf, the same leaf for every `rep_child` column: the leaf closest to the cell's mean over all `rep_child` columns, each divided by its standard deviation (the column's `sd`: over the leaves, or fixed by the writer), and over every element of the `rep_child` vector columns, each divided by its column's one `sd`. Ties go to the lowest cell id. |

With `sd` computed over the leaves, a cell's representative depends on every
leaf of the build, so pyramids of different parts of a region can choose
differently for the same cell. Builds that must agree, such as parts of a
region built one at a time, fix `sd` to the same values.

Missing values are skipped; a cell whose leaves are all missing has a null
value. Values are stored as their column's type at every level: integer types
round half to even and clamp to their range.

### Codecs

A column whose `codec` is set stores *codes*, and its values are the codes
decoded. A `mean` over it decodes each leaf's code, takes the (weighted)
mean of the values and encodes the mean, so a coarse cell holds the code of
the mean of its leaves' values. A nonlinear code does not commute with the
mean: the mean of the codes is not the code of the mean. The leaves hold the
codes the writer was given. A reader decodes a column's codes with its codec.

| Codec | Codes | Decode (code `x` to value) | Encode (value `v` to code) |
|---|---|---|---|
| `aef` | `int8` | `sign(x) * (x / 127.5)^2` | `round(sign(v) * sqrt(abs(v)) * 127.5)`, rounded half away from zero, clamped to -128 to 127 |

`aef` is the code of AlphaEarth Foundations embeddings. Its encoding is that
of the a5embeddings store's cell means, so a coarse cell of a pyramid built
from the store's leaves holds the code the store would write for the mean of
those leaves' decoded embeddings.

A writer takes an `aef` mean as the mean of `x * |x|` (the value times
127.5²) and its code as `round(sign(m) * sqrt(|m|))`, the same arithmetic
in a form that is exact for whole codes and weights. Such means can lie
exactly half way between two codes (codes 2, 2, 1 and 0 average 1.5² when
lifted), and they round away from zero whatever the order of the sum; a mean
of decoded floating-point values would round them either way.

Stored codes are not values: a reader that combines stored cells, such as
partial cells of separate builds that share a coarse cell, decodes them with
the codec, takes their mean weighted by `ncell` (or by the summed weights)
and encodes it, rather than averaging the codes.

## Encodings and compression

- `cell` and `ncell` are `DELTA_BINARY_PACKED`: a tile's ids are sorted and
  close together, so this stores `cell` in about 3 bytes per row rather
  than 8.
- `lod` is dictionary encoded (it is constant in a tile).
- Value columns are `PLAIN`.
- The default compression is Snappy, which hyparquet decodes without extra
  libraries. Writers may use Zstandard (hyparquet needs
  `hyparquet-compressors`) or none.
- No page index is written.

## The Arrow form

A pyramid in memory is a sequence of Arrow record batches with the layout of
the file:

- Each record batch is one tile: batch `i` holds exactly the rows of row
  group `i` of the same pyramid's file, and every rule under "Tiles"
  applies.
- The columns and their Arrow types are those under "Columns".
- The schema metadata holds the entries under "Key-value metadata", the
  index unchanged.

Serialising the batches, for example as Arrow IPC, keeps this layout: an IPC
file's record batches are the tiles, and its schema metadata the index. The
encodings and statistics below are properties of the Parquet file only.

## Pyramid sets

A set stores one pyramid as many files, so that every file's footer stays
small and a view opens only the files around it. A browser reads a file's
whole footer before any tile, and the footer grows by a few hundred bytes a
tile, so one file of 150 million leaves would have a footer of about 20 MB.

### Tiers and shards

The set's LODs are split into *tiers*: ranges of consecutive LODs that do not
overlap and between them hold every level, the leaves included. Each tier has
a *shard resolution* `s`, from 0 up to its coarsest LOD. A cell `C` of the
tier belongs to the *shard* keyed by `cell_to_supercell(C, s)`, the cell at
resolution `s` containing `C`'s centre (`C` itself when `s` is its
resolution). So every cell of the pyramid is in exactly one shard. A shard's
cells lie around its shard cell, and the coarse ones reach past it.

Levels are grouped from all the leaves before the pyramid is split, so a
coarse cell near a shard's edge summarises every leaf under it, whichever
shards hold those leaves.

Each shard is one a5pyramid file (format version 2) holding its tier's LODs
for the cells it owns:

- `levels` has one entry per LOD of the tier that the pyramid has, in
  ascending order, each with the shard's rows at that LOD. A level can be
  empty (`rows` 0, an empty range of tiles); a shard with no cells at all is
  not written.
- `data_resolution` is the leaves' resolution, also in shards whose tier is
  coarser.
- `bbox` covers every tile of the shard.
- `shard` names the tier, the shard cell and its resolution.
- Columns, tiles, encodings and key-value metadata are as for a single file.
  Each shard carries the writer's other metadata entries.

The tiers a writer chooses are its own. The a5pyramid writer's default, from
the leaves down, puts three LODs in a tier with shard resolution 12 below the
tier's finest LOD, and every LOD left over in one tier at shard resolution 0.
A shard full of data then holds about 4^12 (16.8 M) cells at its finest LOD
and a third as many again at its other two, and a map view, about 1,300 cells
across at the LOD it draws, meets at most a few shards of that tier. For
leaves at resolution 18: LODs 16 to 18 (shards at 6), 13 to 15 (at 3) and 2
to 12 (at 0).

### The directory

```
set.json
tier=0/shard=<hex>.parquet
tier=1/shard=<hex>.parquet
...
```

`<hex>` is the shard cell, 16 hex digits. Readers SHOULD take paths from
`set.json` rather than build them. A set has a directory of its own; files
there that `set.json` does not list are not part of it. A writer writes
`set.json` last, so a directory with an index holds every shard it lists.

### set.json

The index of a set of 1,024 leaves at resolution 15 in two tiers, with one
of its six shards shown:

```json
{
  "format": "a5pyramid-set",
  "format_version": 2,
  "generator": "a5pyramid 0.0.0.9000",
  "a5_crate": "1.0.0",
  "a5_index": "1.0",
  "data_resolution": 15,
  "columns": [{"name": "value", "type": "float32", "reducer": "mean"}],
  "tiers": [
    {"lods": [2, 12], "shard_resolution": 0, "tile_size": 16384},
    {"lods": [13, 15], "shard_resolution": 10, "tile_size": 16384}
  ],
  "bbox": [-21.0, 49.98534, 15.0, 71.63832],
  "shards": [
    {"tier": 1, "cell": "6316008000000000", "path": "tier=1/shard=6316008000000000.parquet",
     "rows": 162, "tiles": 3, "bytes": 2687, "footer_bytes": 1404,
     "bbox": [-3.21943, 56.00456, -3.15593, 56.03745]}
  ],
  "metadata": {"app": "{\"k\":1}"}
}
```

| Field | Meaning |
|---|---|
| `format` | `"a5pyramid-set"`. Readers MUST refuse any other. |
| `format_version` | 2, this document's version for sets. Readers MUST refuse newer versions. |
| `generator`, `a5_crate`, `data_resolution`, `columns` | As in a shard's index, the same for every shard. |
| `a5_index` | The A5 index version of every shard's ids, as in each shard's `a5_index` metadata. |
| `tiers` | In ascending LOD order; a tier's number is its position, from 0. `lods` is inclusive. |
| `bbox` | The box of every shard's box. |
| `shards` | Every shard, by tier and then shard cell: `tier`; `cell` (16 hex digits); `path`, relative to the set's directory with `/` separators; `rows`; `tiles`; `bytes`, the file's size; `footer_bytes`; and `bbox`, the shard's box. |
| `metadata` | The writer's other metadata, an object of strings. |

`footer_bytes` counts from the end of the last column chunk to the end of
the file: the Parquet metadata (the shard's index included), its 4-byte
length and the closing `PAR1`. A reader that fetches the last
`footer_bytes` bytes has the whole footer in one request.

### Reading a set

1. Fetch `set.json`. Refuse it if `format` is not `"a5pyramid-set"`,
   `format_version` is newer than the reader supports, or `a5_index` is not
   the reader's A5 index version.
2. Pick a LOD for the view, as for a single file, and the tier holding it.
3. Take the tier's shards whose `bbox` meets the viewport, and open each
   (once; cache it): fetch its last `footer_bytes` bytes and read its index.
4. In each, take the level's tiles whose boxes meet the viewport and fetch
   them, as in a single file. Tiles of different shards never hold the same
   cell.

## Reading with hyparquet

```js
import { asyncBufferFromUrl, parquetMetadataAsync, parquetReadObjects } from "hyparquet";

const file = await asyncBufferFromUrl({ url });
const metadata = await parquetMetadataAsync(file);
const kv = Object.fromEntries(metadata.key_value_metadata.map((e) => [e.key, e.value]));
const index = JSON.parse(kv.a5pyramid);
if (index.format_version > 2 || kv.a5_index !== "1.0") throw new Error("unsupported pyramid");

// row offsets of each tile (row group)
const rowStart = [0];
for (const rg of metadata.row_groups) rowStart.push(rowStart.at(-1) + Number(rg.num_rows));

function intervals([west, , east]) {
  return west > east ? [[west, 180], [-180, east]] : [[west, east]];
}
function meets(a, b) {
  if (a[1] > b[3] || b[1] > a[3]) return false;
  return intervals(a).some(([w1, e1]) => intervals(b).some(([w2, e2]) => w1 <= e2 && w2 <= e1));
}

async function rowsInView(lod, view /* [west, south, east, north] */) {
  const level = index.levels.find((l) => l.lod === lod);
  const out = [];
  for (let rg = level.tiles[0]; rg < level.tiles[1]; rg++) {
    const box = index.tile_bbox.slice(4 * rg, 4 * rg + 4);
    if (!meets(box, view)) continue;
    out.push(await parquetReadObjects({ file, metadata, rowStart: rowStart[rg], rowEnd: rowStart[rg + 1] }));
  }
  return out.flat(); // rows: { cell: BigInt, lod, ncell, ...values }
}
```

A shard opens the same way; with its `footer_bytes` from `set.json`,
`parquetMetadataAsync(file, { initialFetchSize: footer_bytes })` reads the
footer in one request.

The R package's test suite reads files and shards this way under Node
(`tests/testthat/js/read-hyparquet.mjs`) and checks the rows, and that only
the footer and the requested tile's bytes are fetched.
