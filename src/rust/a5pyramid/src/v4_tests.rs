//! Tests of format version 4's features together: list vectors, a weights
//! column, shards written from rows with `owner` and without `lod` or
//! `ncell`, and the shard table.

use std::collections::BTreeMap;
use std::path::PathBuf;

use arrow_array::cast::AsArray;
use arrow_array::types::{Int8Type, UInt32Type, UInt64Type};
use arrow_array::{Array, RecordBatch, StringArray, UInt64Array};
use arrow_schema::DataType;

use crate::testutil::grid;
use crate::{
    build, read_set_table, write_parquet, write_set, write_set_table, Cancel, Codec, Column, ColumnType, Leaves,
    ParquetOptions, Pyramid, PyramidFile, Reducer, SetIndexFormat, SetMeta, SetOptions, ShardEntry, ShardMeta, Tier,
    TileOptions, TiledPyramid, VectorColumn, VectorStorage, WeightsSpec, SET_TABLE,
};

fn tempdir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("a5pyramid-v4-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Leaves at resolution 15 with a 64-element AEF vector and pixel counts.
fn leaves(storage: VectorStorage) -> Leaves {
    let cells = grid(15, (10.0, 10.4), (50.0, 50.25), 160);
    let n = cells.len();
    let bytes: Vec<u8> = (0..n * 64).map(|i| ((i * 37 + i / 64) % 251) as u8).collect();
    let w: Vec<f64> = (0..n).map(|i| (3 + i % 5) as f64).collect();
    Leaves {
        cells,
        vectors: vec![VectorColumn::new("mean_embedding", ColumnType::Int8, 64, bytes)
            .with_reducer(Reducer::Mean)
            .with_codec(Codec::Aef)
            .with_storage(storage)],
        weights: Some(w),
        weights_column: Some(WeightsSpec {
            name: "n_pixels".into(),
            ty: ColumnType::UInt32,
        }),
        ..Default::default()
    }
}

#[test]
fn list_vectors_and_weights_round_trip_in_version_4() {
    let p = build(leaves(VectorStorage::List), &[11, 13], &Cancel::new()).unwrap();
    // each level's weights are its leaves' sums
    let total: f64 = p.levels.last().unwrap().weights.iter().sum();
    for l in &p.levels {
        assert_eq!(l.weights.len(), l.len());
        assert_eq!(l.weights.iter().sum::<f64>(), total, "LOD {}", l.lod);
    }
    let opts = TileOptions {
        tile_size: 100,
        row_group_size: Some(400),
        ..Default::default()
    };
    let tiled = TiledPyramid::new(p.clone(), &opts, &Cancel::new()).unwrap();
    let m = tiled.meta();
    assert_eq!(m.format_version, 4);
    assert_eq!(
        m.weights.as_ref().map(|w| (w.name.as_str(), w.ty.as_str())),
        Some(("n_pixels", "uint32"))
    );
    assert_eq!(m.columns[0].storage.as_deref(), Some("list"));
    let dir = tempdir("lists");
    let path = dir.join("p.parquet");
    // pages of the list column are cut at the tiles too (the write checks)
    let zstd = ParquetOptions {
        compression: crate::Compression::Zstd,
    };
    write_parquet(&tiled, &path, &zstd, &Cancel::new()).unwrap();
    let f = PyramidFile::open(&path).unwrap();
    assert_eq!(f.meta, *m);
    let leaf = p.levels.last().unwrap();
    let tiles = f.select(Some(&[15]), None).unwrap();
    let mut codes = Vec::new();
    let mut weights = Vec::new();
    for b in f.read(&tiles).unwrap() {
        let l = b.column_by_name("mean_embedding").unwrap();
        assert!(matches!(l.data_type(), DataType::List(_)), "{}", l.data_type());
        let l = l.as_list::<i32>();
        for i in 0..l.len() {
            let v = l.value(i);
            assert_eq!(v.len(), 64);
            codes.extend(v.as_primitive::<Int8Type>().values().iter().map(|&x| x as u8));
        }
        weights.extend(
            b.column_by_name("n_pixels")
                .unwrap()
                .as_primitive::<UInt32Type>()
                .values()
                .iter()
                .map(|&x| x as f64),
        );
    }
    assert_eq!(codes, leaf.vectors[0]);
    assert_eq!(weights, leaf.weights);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn list_pages_are_cut_at_tiles_whatever_the_elements_a_row() {
    // 3 elements a row and 100-row tiles: a write batch of 100 levels would
    // end at 102 rows
    let cells = grid(14, (10.0, 10.4), (50.0, 50.25), 120);
    let n = cells.len();
    let leaves = Leaves {
        cells,
        vectors: vec![
            VectorColumn::new("v", ColumnType::UInt8, 3, (0..n * 3).map(|i| (i % 200) as u8).collect())
                .with_storage(VectorStorage::List),
        ],
        ..Default::default()
    };
    let p = build(leaves, &[10], &Cancel::new()).unwrap();
    let opts = TileOptions {
        tile_size: 100,
        row_group_size: Some(700),
        ..Default::default()
    };
    let tiled = TiledPyramid::new(p, &opts, &Cancel::new()).unwrap();
    assert!(tiled.row_groups().iter().any(|g| g.len() > 3));
    let dir = tempdir("list-pages");
    write_parquet(
        &tiled,
        dir.join("p.parquet"),
        &ParquetOptions::default(),
        &Cancel::new(),
    )
    .unwrap();
    let f = PyramidFile::open(dir.join("p.parquet")).unwrap();
    for i in 0..f.n_tiles() {
        assert_eq!(f.read(&[i]).unwrap()[0].num_rows(), tiled.tile_rows()[i]);
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// A partition's level built from its own leaves, as the store writes it.
#[test]
fn shards_from_rows_carry_owner_and_leave_out_lod_and_ncell() {
    let built = build(leaves(VectorStorage::List), &[13], &Cancel::new()).unwrap();
    let shard_res = 9;
    let level = built.level(13).unwrap().clone();
    let partition = a5::cell_to_supercell(level.cells[0], shard_res).unwrap();
    // the rows, scrambled, as a caller hands them over
    let n = level.len();
    let order: Vec<usize> = (0..n).rev().collect();
    let dim = 64;
    let rows = Leaves {
        cells: order.iter().map(|&i| level.cells[i]).collect(),
        vectors: vec![VectorColumn::new(
            "mean_embedding",
            ColumnType::Int8,
            dim,
            order
                .iter()
                .flat_map(|&i| level.vectors[0][i * dim..(i + 1) * dim].to_vec())
                .collect(),
        )
        .with_reducer(Reducer::Mean)
        .with_codec(Codec::Aef)
        .with_storage(VectorStorage::List)],
        weights: Some(order.iter().map(|&i| level.weights[i]).collect()),
        weights_column: built.weights.clone(),
        ..Default::default()
    };
    let ncell: Vec<u32> = order.iter().map(|&i| level.ncell[i]).collect();
    let p = Pyramid::from_rows(rows, Some(ncell), 15).unwrap();
    assert_eq!(p.levels[0], level);
    let opts = TileOptions {
        tile_size: 64,
        shard: Some(ShardMeta {
            tier: 0,
            cell: crate::hex(partition),
            resolution: shard_res,
        }),
        owner: true,
        lod_column: false,
        ..Default::default()
    };
    let tiled = TiledPyramid::new(p, &opts, &Cancel::new()).unwrap();
    assert!(tiled.meta().owner);
    let dir = tempdir("rows");
    let path = dir.join("level.parquet");
    write_parquet(&tiled, &path, &ParquetOptions::default(), &Cancel::new()).unwrap();
    let f = PyramidFile::open(&path).unwrap();
    assert_eq!(f.meta.format_version, 4);
    let batches = f.read(&(0..f.n_tiles()).collect::<Vec<_>>()).unwrap();
    let schema = batches[0].schema();
    let names: Vec<&str> = schema.fields().iter().map(|x| x.name().as_str()).collect();
    assert_eq!(names, ["cell", "ncell", "owner", "n_pixels", "mean_embedding"]);
    for b in &batches {
        let cells = b.column(0).as_primitive::<UInt64Type>();
        let owner = b.column_by_name("owner").unwrap().as_primitive::<UInt64Type>();
        for i in 0..b.num_rows() {
            assert_eq!(
                owner.value(i),
                a5::cell_to_supercell(cells.value(i), shard_res).unwrap()
            );
        }
    }

    // the leaves as a shard of their own: no lod, no ncell
    let leaf = built.levels.last().unwrap().clone();
    let leaf_rows = Leaves {
        cells: leaf.cells.clone(),
        vectors: vec![
            VectorColumn::new("mean_embedding", ColumnType::Int8, dim, leaf.vectors[0].clone())
                .with_reducer(Reducer::Mean)
                .with_codec(Codec::Aef)
                .with_storage(VectorStorage::List),
        ],
        weights: Some(leaf.weights.clone()),
        weights_column: built.weights.clone(),
        ..Default::default()
    };
    let lp = Pyramid::from_rows(leaf_rows, None, 15).unwrap();
    let lopts = TileOptions {
        lod_column: false,
        ncell_column: false,
        ..Default::default()
    };
    let lt = TiledPyramid::new(lp, &lopts, &Cancel::new()).unwrap();
    write_parquet(
        &lt,
        dir.join("leaves.parquet"),
        &ParquetOptions::default(),
        &Cancel::new(),
    )
    .unwrap();
    let lf = PyramidFile::open(dir.join("leaves.parquet")).unwrap();
    let b = lf.read(&[0]).unwrap();
    let schema = b[0].schema();
    let names: Vec<&str> = schema.fields().iter().map(|x| x.name().as_str()).collect();
    assert_eq!(names, ["cell", "n_pixels", "mean_embedding"]);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn rows_and_layouts_that_do_not_fit_are_refused() {
    let one = |cells: Vec<u64>| Leaves {
        cells,
        columns: vec![Column::new("v", ColumnType::Float32, Reducer::Mean, vec![1.0, 2.0])],
        ..Default::default()
    };
    let c = grid(12, (10.0, 10.1), (50.0, 50.1), 4);
    // a duplicate cell; ncell missing above the leaves; ncell 0; weights
    // given without a column to store them in
    assert!(Pyramid::from_rows(one(vec![c[0], c[0]]), Some(vec![1, 1]), 14).is_err());
    assert!(Pyramid::from_rows(one(vec![c[0], c[1]]), None, 14).is_err());
    assert!(Pyramid::from_rows(one(vec![c[0], c[1]]), Some(vec![0, 1]), 14).is_err());
    assert!(Pyramid::from_rows(one(vec![c[0], c[1]]), Some(vec![2, 1]), 12).is_err());
    let mut w = one(vec![c[0], c[1]]);
    w.weights = Some(vec![1.0, 1.0]);
    assert!(Pyramid::from_rows(w, Some(vec![1, 1]), 14).is_err());
    let p = Pyramid::from_rows(one(vec![c[1], c[0]]), Some(vec![3, 1]), 14).unwrap();
    assert_eq!(p.levels[0].ncell, if c[0] < c[1] { vec![1, 3] } else { vec![3, 1] });

    let cancel = Cancel::new();
    let owner_alone = TileOptions {
        owner: true,
        ..Default::default()
    };
    assert!(TiledPyramid::new(p.clone(), &owner_alone, &cancel).is_err());
    let no_ncell = TileOptions {
        ncell_column: false,
        ..Default::default()
    };
    assert!(TiledPyramid::new(p, &no_ncell, &cancel).is_err());
    let two = build(leaves(VectorStorage::FixedBinary), &[12], &cancel).unwrap();
    let no_lod = TileOptions {
        lod_column: false,
        ..Default::default()
    };
    assert!(TiledPyramid::new(two, &no_lod, &cancel).is_err());
}

#[test]
fn a_set_with_a_parquet_index_reads_back_whole() {
    let p = build(leaves(VectorStorage::List), &[9, 11, 13], &Cancel::new()).unwrap();
    let dir = tempdir("set-table");
    let opts = SetOptions {
        tiers: vec![
            Tier {
                lods: [9, 11],
                shard_resolution: 0,
                tile_size: 50,
                row_group_size: None,
            },
            Tier {
                lods: [13, 15],
                shard_resolution: 9,
                tile_size: 100,
                row_group_size: Some(400),
            },
        ],
        generator: "test".into(),
        metadata: vec![("app".into(), "1".into())],
        parquet: ParquetOptions::default(),
        index: SetIndexFormat::Parquet,
    };
    let meta = write_set(p, &dir, &opts, &Cancel::new()).unwrap();
    assert_eq!(meta.format_version, 4);
    assert!(dir.join(SET_TABLE).exists() && !dir.join(crate::SET_INDEX).exists());
    assert_eq!(SetMeta::read(&dir).unwrap(), meta);
    assert_eq!(SetMeta::read(dir.join(SET_TABLE)).unwrap(), meta);
    assert!(meta.weights.is_some());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn shard_tables_list_empty_shards_and_writer_columns_with_unwrapped_boxes() {
    let p = build(leaves(VectorStorage::List), &[13], &Cancel::new()).unwrap();
    let dir = tempdir("assemble");
    let tiled = TiledPyramid::new(
        p,
        &TileOptions {
            shard: Some(ShardMeta {
                tier: 1,
                cell: "0".repeat(16),
                resolution: 0,
            }),
            ..Default::default()
        },
        &Cancel::new(),
    )
    .unwrap();
    let path = dir.join("a.parquet");
    let s = write_parquet(&tiled, &path, &ParquetOptions::default(), &Cancel::new()).unwrap();
    let f = PyramidFile::open(&path).unwrap();
    let tiers = vec![
        Tier {
            lods: [13, 13],
            shard_resolution: 6,
            tile_size: 4096,
            row_group_size: None,
        },
        Tier {
            lods: [15, 15],
            shard_resolution: 6,
            tile_size: 4096,
            row_group_size: None,
        },
    ];
    let entry = |cell: &str, path: Option<&str>, bbox: Option<[f64; 4]>| ShardEntry {
        tier: 1,
        cell: cell.into(),
        path: path.map(Into::into),
        rows: if path.is_some() { s.rows } else { 0 },
        tiles: if path.is_some() { s.tiles } else { 0 },
        bytes: if path.is_some() { s.bytes } else { 0 },
        footer_bytes: if path.is_some() { f.footer_bytes() } else { 0 },
        bbox,
    };
    let shards = vec![
        entry("0a00000000000000", Some("a.parquet"), Some(tiled.meta().bbox)),
        entry("0b00000000000000", None, None),
        // a box across the antimeridian
        entry("0c00000000000000", Some("a.parquet"), Some([179.0, -1.0, -179.0, 1.0])),
    ];
    let meta = SetMeta::assemble(&f, tiers.clone(), shards.clone(), "store".into(), BTreeMap::new()).unwrap();
    assert_eq!(meta.format_version, 4);
    let extra = RecordBatch::try_from_iter([
        (
            "year",
            std::sync::Arc::new(UInt64Array::from(vec![2021u64, 2021, 2021])) as _,
        ),
        (
            "build",
            std::sync::Arc::new(StringArray::from(vec!["b1", "b1", "b2"])) as _,
        ),
    ])
    .unwrap();
    let table = dir.join("set.parquet");
    write_set_table(&meta, Some(&extra), &table, &ParquetOptions::default()).unwrap();
    let (back, cols) = read_set_table(&table).unwrap();
    assert_eq!(back, meta);
    assert_eq!(cols, extra);
    // stored unwrapped, so row-group statistics bound the boxes
    let raw =
        parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder::try_new(std::fs::File::open(&table).unwrap())
            .unwrap()
            .build()
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
    let east = raw
        .column_by_name("east")
        .unwrap()
        .as_primitive::<arrow_array::types::Float64Type>();
    assert_eq!(east.value(2), 181.0);
    assert!(east.is_null(1));
    // only shards with files are selected
    assert_eq!(back.select(1, None), vec![0, 2]);

    // out of order, an empty shard with a file's size, a misnamed extra column
    let mut bad = meta.clone();
    bad.shards.swap(0, 1);
    assert!(write_set_table(&bad, None, dir.join("x.parquet"), &ParquetOptions::default()).is_err());
    let mut bad = meta.clone();
    bad.shards[1].rows = 5;
    assert!(write_set_table(&bad, None, dir.join("x.parquet"), &ParquetOptions::default()).is_err());
    let clash =
        RecordBatch::try_from_iter([("rows", std::sync::Arc::new(UInt64Array::from(vec![1u64, 2, 3])) as _)]).unwrap();
    assert!(write_set_table(&meta, Some(&clash), dir.join("x.parquet"), &ParquetOptions::default()).is_err());
    assert!(!dir.join("x.parquet").exists());
    std::fs::remove_dir_all(&dir).unwrap();
}
