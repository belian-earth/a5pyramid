// Read one tile of an a5pyramid file as a browser does, with hyparquet: the
// footer first, then that tile's rows only, through an AsyncBuffer that logs
// every byte range it serves (as a range-request buffer over HTTP). Row group
// g of n rows holds ceil(n / tile_size) tiles where the index has a
// row_group_size (format version 4), each one data page of every column
// chunk, which hyparquet finds through the offset index; otherwise it is one
// tile.
//
//   node read-hyparquet-tile.mjs <hyparquet.esm.js> <file.parquet> <tile>
//
// Prints JSON: the tile's place, its decoded rows and the bytes fetched.
import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

const [hyparquetPath, file, tileArg] = process.argv.slice(2);
const hp = await import(pathToFileURL(hyparquetPath).href);

const bytes = readFileSync(file);
const fetched = [];
const asyncBuffer = {
  byteLength: bytes.byteLength,
  slice(start, end = bytes.byteLength) {
    fetched.push([start, end]);
    return Promise.resolve(bytes.buffer.slice(bytes.byteOffset + start, bytes.byteOffset + end));
  },
};

const metadata = await hp.parquetMetadataAsync(asyncBuffer);
const footerFetches = fetched.length;
const kv = Object.fromEntries((metadata.key_value_metadata || []).map((e) => [e.key, e.value]));
const index = JSON.parse(kv.a5pyramid);

// where each tile is, from the footer alone
const tiles = [];
let first = 0;
metadata.row_groups.forEach((rg, g) => {
  const n = Number(rg.num_rows);
  const k = index.row_group_size ? Math.ceil(n / index.tile_size) : 1;
  const step = k === 1 ? n : index.tile_size;
  for (let p = 0; p < k; p++) tiles.push({ g, page: p, start: first + p * step, rows: Math.min(step, n - p * step) });
  first += n;
});
const t = tiles[Number(tileArg)];
const rows = await hp.parquetReadObjects({
  file: asyncBuffer, metadata, rowStart: t.start, rowEnd: t.start + t.rows, useOffsetIndex: true,
});

const hex = (v) => (typeof v === "bigint" ? v : BigInt(v)).toString(16).padStart(16, "0");
const out = {
  format_version: index.format_version,
  n_tiles: tiles.length,
  n_row_groups: metadata.row_groups.length,
  row_group: t.g,
  page: t.page,
  cells: rows.map((r) => hex(r.cell)),
  lod: rows.map((r) => Number(r.lod)),
  ncell: rows.map((r) => Number(r.ncell)),
  columns: Object.fromEntries(
    index.columns
      .filter((c) => c.dim == null)
      .map((c) => [c.name, rows.map((r) => (r[c.name] == null ? null : Number(r[c.name])))]),
  ),
  row_group_bytes: metadata.row_groups[t.g].columns.reduce(
    (a, c) => a + Number(c.meta_data.total_compressed_size), 0),
  fetched_bytes: fetched.slice(footerFetches).reduce((a, [s, e]) => a + (e - s), 0),
};
process.stdout.write(JSON.stringify(out));
