# The a5pyramid file format, version 1

An a5pyramid file is one Parquet file holding [A5](https://a5geo.org/) cells
at one resolution (the *leaves*, with their values) and coarser *levels of
detail* (LODs) aggregated from them. It is laid out so that a client, such as
a web map reading it with [hyparquet](https://github.com/hyparam/hyparquet),
fetches the footer and then only the row groups a view needs, with HTTP range
requests.

The same layout also exists in memory as Arrow record batches (see "The
Arrow form"), for clients that take a pyramid without a file.

This document is the contract between writers (the `a5pyramid` crate and the
R package built on it) and readers (renderers). Key words follow RFC 2119.

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
| value columns | see below | see below | One per column in the index's `columns`, in that order. Null where missing. |

Value column types (`type` in the index) are `int8`, `int16`, `int32`,
`uint8`, `uint16`, `uint32`, `float32` and `float64`, stored as the matching
Parquet `INT32`/`FLOAT`/`DOUBLE` physical type with an `INTEGER` logical type
for the integer types. hyparquet decodes `cell` as `BigInt` and every other
column as `Number`.

The names `cell`, `lod` and `ncell` are reserved: no value column takes them.

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
| `format_version` | This document's version. Readers MUST refuse newer versions. Fields added without a version change are optional, and readers MUST ignore fields they do not know. |
| `generator` | The software that wrote the file. |
| `a5_crate` | The version of the a5 Rust crate that computed the levels and boxes. |
| `data_resolution` | The leaves' resolution, the finest level's `lod`. |
| `tile_size` | The most rows in a tile. |
| `bbox` | `[west, south, east, north]` of the leaves (see "Boxes"). |
| `columns` | The value columns in column order: `name`, storage `type` and `reducer` (see "Levels"). |
| `levels` | One entry per level, ascending `lod`: its `lod`, its number of `rows`, and the half-open range `[start, end)` of its tiles. Consecutive levels' ranges meet, and together cover every tile. |
| `tile_bbox` | The box of every tile in order, flat: tile `i` is at indices `4i` to `4i + 3`. |

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
below would place some leaves in the wrong coarse cell.

Each value column is reduced over the leaves under each cell by its
`reducer`:

| Reducer | Value of a coarse cell |
|---|---|
| `mean` | The mean, weighted by the leaves' weights when the writer had them. |
| `sum`, `min`, `max` | The sum, minimum or maximum. |
| `mode` | The most frequent value (by total weight), ties to the smallest. |
| `rep_child` | The value of one *representative* leaf, the same leaf for every `rep_child` column: the leaf closest to the cell's mean over all `rep_child` columns, each scaled by its standard deviation over the leaves. Ties go to the lowest cell id. |

Missing values are skipped; a cell whose leaves are all missing has a null
value. Values are stored as their column's type at every level: integer types
round half to even and clamp to their range.

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

## Reading with hyparquet

```js
import { asyncBufferFromUrl, parquetMetadataAsync, parquetReadObjects } from "hyparquet";

const file = await asyncBufferFromUrl({ url });
const metadata = await parquetMetadataAsync(file);
const kv = Object.fromEntries(metadata.key_value_metadata.map((e) => [e.key, e.value]));
const index = JSON.parse(kv.a5pyramid);
if (index.format_version > 1 || kv.a5_index !== "1.0") throw new Error("unsupported pyramid");

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

The R package's test suite reads files this way under Node
(`tests/testthat/js/read-hyparquet.mjs`) and checks the rows, and that only
the footer and the requested tile's bytes are fetched.
