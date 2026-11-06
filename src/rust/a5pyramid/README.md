# a5pyramid

Level-of-detail pyramids of [A5](https://a5geo.org/) cells, written to one
Parquet file for browser rendering.

`build()` aggregates cells at one resolution to coarser levels of detail,
grouping by the coarse cell containing each cell's centre, with mean, sum,
min, max, mode or representative-leaf reducers. `write_parquet()` tiles each
level into row groups and indexes their levels and bounding boxes in the
file's metadata, so a web map can fetch only the tiles in view with HTTP
range requests. `PyramidFile` reads the index and selected row groups back.

```rust
use a5pyramid::{build, default_lods, write_parquet, Cancel, Column, ColumnType, Leaves, Reducer, WriteOptions};

let leaves = Leaves {
    cells,  // Vec<u64>, one resolution
    columns: vec![Column::new("value", ColumnType::Float32, Reducer::Mean, values)],
    weights: None,
};
let pyramid = build(leaves, &default_lods(16), &Cancel::new())?;
write_parquet(&pyramid, "pyramid.parquet", &WriteOptions::default(), &Cancel::new())?;
```

The file layout is specified in [FORMAT.md](FORMAT.md). The crate is
developed inside the [a5pyramid R package](https://github.com/belian-earth/a5pyramid)
and will be published once the format settles. `examples/bench.rs` times a
build:

```sh
cargo run -p a5pyramid --release --example bench -- 16
```
