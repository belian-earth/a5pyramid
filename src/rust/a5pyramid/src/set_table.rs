//! A set's index as a Parquet table (format version 4): one row per shard,
//! the set's own fields in the footer.
//!
//! `set.json` lists every shard in one document, which a reader fetches
//! whole. A store of tens of thousands of shards, or a service that hands
//! a client some of them, wants a table instead: readers filter it by tier
//! and box with row-group statistics, and a writer can carry columns of its
//! own beside the format's (a build name, a year, an etag). Boxes are
//! unwrapped, `east` above 180 for a box across the antimeridian, so a row
//! group's min and max always bound its rows.

use std::fs::File;
use std::path::Path;
use std::sync::Arc;

use arrow_array::cast::AsArray;
use arrow_array::types::{Float64Type, UInt16Type, UInt32Type, UInt64Type};
use arrow_array::{Array, ArrayRef, Float64Array, RecordBatch, StringArray, UInt16Array, UInt32Array, UInt64Array};
use arrow_schema::{DataType, Field, Schema};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::arrow::arrow_writer::ArrowWriterOptions;
use parquet::arrow::ArrowWriter;
use parquet::file::metadata::KeyValue;
use parquet::file::properties::{EnabledStatistics, WriterProperties, WriterVersion};
use parquet::schema::types::ColumnPath;

use crate::error::{invalid, Error, Result};
use crate::meta::{A5_INDEX_KEY, VERSION_4};
use crate::set::{SetMeta, ShardEntry, SET_FORMAT};
use crate::write::ParquetOptions;

/// File name of a set's index as a table, in the set's directory.
pub const SET_TABLE: &str = "set.parquet";
/// Key-value metadata key of the set's own fields (its `set.json` without
/// `shards`), in the table's footer.
pub const SET_META_KEY: &str = "a5pyramid_set";

/// The table's columns, in order; a writer's own follow them.
pub const SET_TABLE_COLUMNS: [&str; 11] = [
    "tier",
    "cell",
    "path",
    "rows",
    "tiles",
    "bytes",
    "footer_bytes",
    "west",
    "south",
    "east",
    "north",
];

/// Rows a row group of the table holds.
const TABLE_ROW_GROUP: usize = 1024;

fn schema(extra: Option<&RecordBatch>) -> Schema {
    let mut fields = vec![
        Field::new("tier", DataType::UInt16, false),
        Field::new("cell", DataType::UInt64, false),
        Field::new("path", DataType::Utf8, true),
        Field::new("rows", DataType::UInt64, false),
        Field::new("tiles", DataType::UInt32, false),
        Field::new("bytes", DataType::UInt64, false),
        Field::new("footer_bytes", DataType::UInt64, false),
        Field::new("west", DataType::Float64, true),
        Field::new("south", DataType::Float64, true),
        Field::new("east", DataType::Float64, true),
        Field::new("north", DataType::Float64, true),
    ];
    if let Some(x) = extra {
        fields.extend(x.schema().fields().iter().map(|f| f.as_ref().clone()));
    }
    Schema::new(fields)
}

/// Check that a set's shards can be listed: by tier and then cell, each
/// once, in a tier the set has, and a shard without a file empty and
/// without a box.
pub(crate) fn check_shards(meta: &SetMeta) -> Result<()> {
    let mut last: Option<(usize, u64)> = None;
    for s in &meta.shards {
        let cell = u64::from_str_radix(&s.cell, 16)
            .map_err(|_| invalid(format!("shard cell {:?} is not 16 hex digits", s.cell)))?;
        if s.tier >= meta.tiers.len() {
            return Err(invalid(format!(
                "a shard of tier {} in a set of {} tiers",
                s.tier,
                meta.tiers.len()
            )));
        }
        if last.is_some_and(|l| l >= (s.tier, cell)) {
            return Err(invalid(format!(
                "shards must be listed by tier and then cell, each once: tier {} cell {} is out of order",
                s.tier, s.cell
            )));
        }
        last = Some((s.tier, cell));
        match (&s.path, &s.bbox) {
            (Some(_), Some(_)) => {}
            (None, None) if s.rows == 0 && s.tiles == 0 => {}
            (None, _) => {
                return Err(invalid(format!(
                    "shard {} of tier {} has no file, so it must be empty, with no tiles and no box",
                    s.cell, s.tier
                )))
            }
            (Some(_), None) => {
                return Err(invalid(format!(
                    "shard {} of tier {} has a file but no box",
                    s.cell, s.tier
                )))
            }
        }
    }
    Ok(())
}

/// Write `meta` as a shard table at `path`, with the columns of `extra` (one
/// row per shard, in `meta.shards` order) after the format's. The table is
/// version 4, whatever `meta.format_version` says.
pub fn write_set_table(
    meta: &SetMeta,
    extra: Option<&RecordBatch>,
    path: impl AsRef<Path>,
    opts: &ParquetOptions,
) -> Result<()> {
    check_shards(meta)?;
    let n = meta.shards.len();
    if let Some(x) = extra {
        if x.num_rows() != n {
            return Err(invalid(format!(
                "{} rows of extra columns for {n} shards",
                x.num_rows()
            )));
        }
        if let Some(f) = x
            .schema()
            .fields()
            .iter()
            .find(|f| SET_TABLE_COLUMNS.contains(&f.name().as_str()))
        {
            return Err(invalid(format!(
                "the extra column {:?} is one of the table's own",
                f.name()
            )));
        }
    }
    let mut head = meta.clone();
    head.format_version = head.format_version.max(VERSION_4);
    head.shards = Vec::new();
    let s = &meta.shards;
    let side = |k: usize| -> ArrayRef {
        Arc::new(Float64Array::from(
            s.iter()
                .map(|e| {
                    e.bbox.map(|b| {
                        // unwrapped: east above 180 across the antimeridian
                        if k == 2 && b[0] > b[2] {
                            b[2] + 360.0
                        } else {
                            b[k]
                        }
                    })
                })
                .collect::<Vec<Option<f64>>>(),
        ))
    };
    let cells = s
        .iter()
        .map(|e| u64::from_str_radix(&e.cell, 16))
        .collect::<std::result::Result<Vec<u64>, _>>()
        .map_err(|_| invalid("a shard cell is not 16 hex digits"))?;
    let mut columns: Vec<ArrayRef> = vec![
        Arc::new(UInt16Array::from(s.iter().map(|e| e.tier as u16).collect::<Vec<_>>())),
        Arc::new(UInt64Array::from(cells)),
        Arc::new(StringArray::from(
            s.iter().map(|e| e.path.clone()).collect::<Vec<Option<String>>>(),
        )),
        Arc::new(UInt64Array::from(s.iter().map(|e| e.rows).collect::<Vec<_>>())),
        Arc::new(UInt32Array::from(s.iter().map(|e| e.tiles as u32).collect::<Vec<_>>())),
        Arc::new(UInt64Array::from(s.iter().map(|e| e.bytes).collect::<Vec<_>>())),
        Arc::new(UInt64Array::from(s.iter().map(|e| e.footer_bytes).collect::<Vec<_>>())),
        side(0),
        side(1),
        side(2),
        side(3),
    ];
    if let Some(x) = extra {
        columns.extend(x.columns().iter().cloned());
    }
    let schema = Arc::new(schema(extra));
    let batch = RecordBatch::try_new(Arc::clone(&schema), columns)?;
    let kv = vec![
        KeyValue::new(SET_META_KEY.to_string(), head.to_json()?),
        KeyValue::new(A5_INDEX_KEY.to_string(), meta.a5_index.clone()),
    ];
    let mut props = WriterProperties::builder()
        .set_writer_version(WriterVersion::PARQUET_1_0)
        .set_compression(opts.compression.to_parquet())
        .set_statistics_enabled(EnabledStatistics::None)
        .set_max_row_group_row_count(Some(TABLE_ROW_GROUP))
        .set_key_value_metadata(Some(kv));
    for c in ["tier", "west", "south", "east", "north"] {
        props = props.set_column_statistics_enabled(ColumnPath::from(c), EnabledStatistics::Chunk);
    }
    let path = path.as_ref();
    let tmp = path.with_extension(format!("parquet.{}.partial", std::process::id()));
    let out = (|| -> Result<()> {
        let options = ArrowWriterOptions::new()
            .with_properties(props.build())
            .with_skip_arrow_metadata(true);
        let mut w = ArrowWriter::try_new_with_options(File::create(&tmp)?, schema, options)?;
        w.write(&batch)?;
        w.close()?;
        std::fs::rename(&tmp, path)?;
        Ok(())
    })();
    if out.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    out
}

/// Read a shard table: the set, its shards (boxes wrapped again, `west >
/// east` across the antimeridian), and the writer's own columns.
pub fn read_set_table(path: impl AsRef<Path>) -> Result<(SetMeta, RecordBatch)> {
    let path = path.as_ref();
    let builder = ParquetRecordBatchReaderBuilder::try_new(File::open(path)?)?;
    let kv = builder
        .metadata()
        .file_metadata()
        .key_value_metadata()
        .cloned()
        .unwrap_or_default();
    let head = kv
        .iter()
        .find(|e| e.key == SET_META_KEY)
        .and_then(|e| e.value.clone())
        .ok_or_else(|| {
            Error::Format(format!(
                "{} is not an a5pyramid shard table: no {SET_META_KEY:?} metadata",
                path.display()
            ))
        })?;
    let mut meta = SetMeta::from_json(&head)?;
    if meta.format != SET_FORMAT {
        return Err(Error::Format(format!("not an a5pyramid set: format {:?}", meta.format)));
    }
    let schema = builder.schema().clone();
    let batches = builder.build()?.collect::<std::result::Result<Vec<_>, _>>()?;
    let batch = if batches.is_empty() {
        RecordBatch::new_empty(schema.clone())
    } else {
        concat(&schema, &batches)?
    };
    let col = |name: &str| -> Result<&ArrayRef> {
        batch
            .column_by_name(name)
            .ok_or_else(|| Error::Format(format!("the shard table has no {name:?} column")))
    };
    let tier = col("tier")?
        .as_primitive_opt::<UInt16Type>()
        .ok_or_else(|| Error::Format("tier must be uint16".into()))?;
    let cell = col("cell")?
        .as_primitive_opt::<UInt64Type>()
        .ok_or_else(|| Error::Format("cell must be uint64".into()))?;
    let path_col = col("path")?
        .as_string_opt::<i32>()
        .ok_or_else(|| Error::Format("path must be a string".into()))?;
    let u64s = |name: &str| -> Result<&UInt64Array> {
        col(name)?
            .as_primitive_opt::<UInt64Type>()
            .ok_or_else(|| Error::Format(format!("{name} must be uint64")))
    };
    let (rows, bytes, footer) = (u64s("rows")?, u64s("bytes")?, u64s("footer_bytes")?);
    let tiles = col("tiles")?
        .as_primitive_opt::<UInt32Type>()
        .ok_or_else(|| Error::Format("tiles must be uint32".into()))?;
    let f64s = |name: &str| -> Result<&Float64Array> {
        col(name)?
            .as_primitive_opt::<Float64Type>()
            .ok_or_else(|| Error::Format(format!("{name} must be double")))
    };
    let sides = [f64s("west")?, f64s("south")?, f64s("east")?, f64s("north")?];
    meta.shards = (0..batch.num_rows())
        .map(|i| {
            let bbox = sides[0].is_valid(i).then(|| {
                let mut b = [
                    sides[0].value(i),
                    sides[1].value(i),
                    sides[2].value(i),
                    sides[3].value(i),
                ];
                if b[2] > 180.0 {
                    b[2] -= 360.0;
                }
                b
            });
            ShardEntry {
                tier: tier.value(i) as usize,
                cell: crate::hex(cell.value(i)),
                path: path_col.is_valid(i).then(|| path_col.value(i).to_string()),
                rows: rows.value(i),
                tiles: tiles.value(i) as usize,
                bytes: bytes.value(i),
                footer_bytes: footer.value(i),
                bbox,
            }
        })
        .collect();
    check_shards(&meta).map_err(|e| Error::Format(format!("inconsistent shard table: {e}")))?;
    let extra: Vec<usize> = (0..schema.fields().len())
        .filter(|&k| !SET_TABLE_COLUMNS.contains(&schema.field(k).name().as_str()))
        .collect();
    Ok((meta, batch.project(&extra)?))
}

/// One batch of `batches`, column by column.
fn concat(schema: &Arc<Schema>, batches: &[RecordBatch]) -> Result<RecordBatch> {
    if batches.len() == 1 {
        return Ok(batches[0].clone());
    }
    let columns = (0..schema.fields().len())
        .map(|k| {
            let parts: Vec<&dyn Array> = batches.iter().map(|b| b.column(k).as_ref()).collect();
            arrow_select_concat(&parts)
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(RecordBatch::try_new(Arc::clone(schema), columns)?)
}

fn arrow_select_concat(parts: &[&dyn Array]) -> Result<ArrayRef> {
    Ok(arrow_select::concat::concat(parts)?)
}
