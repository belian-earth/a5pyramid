//! Build and write a pyramid over a box of leaves, timing each step.
//!
//! cargo run -p a5pyramid --release --example bench -- [res] [threads] [west south east north]
use std::time::Instant;

use a5pyramid::{
    build, default_lods, write_parquet, Cancel, Column, ColumnType, Leaves, ParquetOptions, PyramidFile, Reducer,
    TileOptions, TiledPyramid,
};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let res: i32 = args.get(1).map_or(16, |s| s.parse().unwrap());
    let threads: usize = args.get(2).map_or(0, |s| s.parse().unwrap());
    let b: Vec<f64> = if args.len() >= 7 {
        args[3..7].iter().map(|s| s.parse().unwrap()).collect()
    } else {
        vec![-3.6, 55.7, -2.6, 56.2]
    };
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
    pool.install(|| {
        let ring = vec![
            a5::LonLat::new(b[0], b[1]),
            a5::LonLat::new(b[2], b[1]),
            a5::LonLat::new(b[2], b[3]),
            a5::LonLat::new(b[0], b[3]),
        ];
        let cells = a5::uncompact(&a5::polygon_to_cells(&[ring], res, None).unwrap()).unwrap();
        let n = cells.len();
        println!("leaves {n} at res {res}, {} threads", rayon::current_num_threads());
        let rgba = |k: usize| -> Vec<f64> { (0..n).map(|i| ((i * (k + 31) * 2654435761) % 256) as f64).collect() };
        let value: Vec<f64> = (0..n).map(|i| (i as f64 * 0.001).sin()).collect();
        for reducer in [Reducer::Mean, Reducer::RepChild] {
            let columns = vec![
                Column::new("r", ColumnType::UInt8, reducer, rgba(0)),
                Column::new("g", ColumnType::UInt8, reducer, rgba(1)),
                Column::new("b", ColumnType::UInt8, reducer, rgba(2)),
                Column::new("value", ColumnType::Float32, reducer, value.clone()),
            ];
            let leaves = Leaves {
                cells: cells.clone(),
                columns,
                weights: None,
            };
            let t = Instant::now();
            let p = build(leaves, &default_lods(res), &Cancel::new()).unwrap();
            let t_build = t.elapsed();
            let path = std::env::temp_dir().join("a5pyramid-bench.parquet");
            let t = Instant::now();
            let tiled = TiledPyramid::new(p, &TileOptions::default(), &Cancel::new()).unwrap();
            let t_tile = t.elapsed();
            let t = Instant::now();
            let batches: usize = tiled.batches().map(|b| b.unwrap().num_rows()).sum();
            let t_arrow = t.elapsed();
            let t = Instant::now();
            let s = write_parquet(&tiled, &path, &ParquetOptions::default(), &Cancel::new()).unwrap();
            let t_write = t.elapsed();
            assert_eq!(batches as u64, s.rows);
            let f = PyramidFile::open(&path).unwrap();
            println!(
                "{:>9}: build {:.3}s tile {:.3}s arrow {:.3}s parquet {:.3}s | rows {} tiles {} file {:.2} MB footer {:.1} KB",
                reducer.as_str(),
                t_build.as_secs_f64(),
                t_tile.as_secs_f64(),
                t_arrow.as_secs_f64(),
                t_write.as_secs_f64(),
                s.rows,
                f.n_row_groups(),
                s.bytes as f64 / 1e6,
                f.footer_bytes() as f64 / 1e3
            );
        }
    });
}
