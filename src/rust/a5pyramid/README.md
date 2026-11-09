# a5pyramid

Level-of-detail pyramids of [A5](https://a5geo.org/) cells, written to one
Parquet file for browser rendering.

`build()` aggregates cells at one resolution to coarser levels of detail,
grouping by the coarse cell containing each cell's centre, with mean, sum,
min, max, mode or representative-leaf reducers. `TiledPyramid` splits each
level into tiles of nearby cells, one Arrow record batch each, and indexes
every tile's level and bounding box in the schema metadata. That is the
pyramid in memory, and `into_reader()` streams it as an arrow-rs
`RecordBatchReader`. `write_parquet()` writes the same tiles as row groups of
one Parquet file, so a web map can fetch only the tiles in view with HTTP
range requests. `PyramidFile` reads the index and selected row groups back.

```rust
use a5pyramid::{
    build, default_lods, write_parquet, Cancel, Column, ColumnType, Leaves, ParquetOptions, Reducer,
    TileOptions, TiledPyramid,
};
use arrow_array::RecordBatchReader;

let leaves = Leaves {
    cells, // Vec<u64>, one resolution
    columns: vec![Column::new("value", ColumnType::Float32, Reducer::Mean, values)],
    weights: None,
};
let pyramid = build(leaves, &default_lods(16), &Cancel::new())?;
let tiled = TiledPyramid::new(pyramid, &TileOptions::default(), &Cancel::new())?;

// in memory: one record batch per tile, the index in the schema metadata
let in_view = tiled.select(Some(&[12]), Some(&bbox))?;
let batch = tiled.tile(in_view[0])?;

// or on disk: the same tiles as row groups
write_parquet(&tiled, "pyramid.parquet", &ParquetOptions::default(), &Cancel::new())?;

// or to any Arrow consumer, here an IPC file
let reader = tiled.into_reader();
let mut w = arrow_ipc::writer::FileWriter::try_new(file, &reader.schema())?;
for batch in reader {
    w.write(&batch?)?;
}
w.finish()?;
```

The file layout is specified in [FORMAT.md](FORMAT.md). The crate is
developed inside the [a5pyramid R package](https://github.com/belian-earth/a5pyramid)
and will be published once the format settles. `examples/bench.rs` times a
build:

```sh
cargo run -p a5pyramid --release --example bench -- 16
```
