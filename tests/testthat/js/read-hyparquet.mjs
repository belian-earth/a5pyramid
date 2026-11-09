// Read one row group of an a5pyramid file as a browser does, with hyparquet:
// the footer first, then that row group only, through an AsyncBuffer that
// logs every byte range it serves (as a range-request buffer over HTTP).
//
//   node read-hyparquet.mjs <hyparquet.esm.js> <file.parquet> <row group>
//
// Prints JSON: the index, the decoded rows and the bytes fetched.
import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

const [hyparquetPath, file, rgArg] = process.argv.slice(2);
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

const rg = Number(rgArg);
let rowStart = 0;
for (let i = 0; i < rg; i++) rowStart += Number(metadata.row_groups[i].num_rows);
const rowEnd = rowStart + Number(metadata.row_groups[rg].num_rows);
const rows = await hp.parquetReadObjects({ file: asyncBuffer, metadata, rowStart, rowEnd });

const hex = (v) => (typeof v === "bigint" ? v : BigInt(v)).toString(16).padStart(16, "0");
const level = index.levels.find((l) => rg >= l.tiles[0] && rg < l.tiles[1]);
const out = {
  a5_index: kv.a5_index,
  format_version: index.format_version,
  lods: index.levels.map((l) => l.lod),
  level_lod: level.lod,
  n_row_groups: metadata.row_groups.length,
  bbox: index.tile_bbox.slice(4 * rg, 4 * rg + 4),
  cell_type: typeof rows[0].cell,
  cells: rows.map((r) => hex(r.cell)),
  lod: rows.map((r) => Number(r.lod)),
  ncell: rows.map((r) => Number(r.ncell)),
  columns: Object.fromEntries(
    index.columns.map((c) => [c.name, rows.map((r) => (r[c.name] == null ? null : Number(r[c.name])))]),
  ),
  footer_fetches: footerFetches,
  row_group_bytes: metadata.row_groups[rg].columns.reduce(
    (a, c) => a + Number(c.meta_data.total_compressed_size), 0),
  fetched_bytes: fetched.slice(footerFetches).reduce((a, [s, e]) => a + (e - s), 0),
};
process.stdout.write(JSON.stringify(out));
