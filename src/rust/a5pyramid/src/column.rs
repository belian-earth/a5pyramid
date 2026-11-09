use arrow_schema::DataType;

use crate::error::{invalid, Result};

/// Storage type of a value column in the Parquet file. Every level shares
/// one schema, so aggregates take the type of their column: means of an
/// integer column are rounded (half to even, as R's `round()`) and clamped
/// to the type's range, at every level including the leaves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColumnType {
    Int8,
    Int16,
    Int32,
    UInt8,
    UInt16,
    UInt32,
    Float32,
    Float64,
}

impl ColumnType {
    pub const ALL: [ColumnType; 8] = [
        Self::Int8,
        Self::Int16,
        Self::Int32,
        Self::UInt8,
        Self::UInt16,
        Self::UInt32,
        Self::Float32,
        Self::Float64,
    ];

    pub fn parse(s: &str) -> Result<Self> {
        Self::ALL.into_iter().find(|t| t.as_str() == s).ok_or_else(|| {
            let names: Vec<&str> = Self::ALL.iter().map(|t| t.as_str()).collect();
            invalid(format!(
                "unknown column type {s:?}; expected one of {}",
                names.join(", ")
            ))
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Int8 => "int8",
            Self::Int16 => "int16",
            Self::Int32 => "int32",
            Self::UInt8 => "uint8",
            Self::UInt16 => "uint16",
            Self::UInt32 => "uint32",
            Self::Float32 => "float32",
            Self::Float64 => "float64",
        }
    }

    pub fn is_integer(self) -> bool {
        !matches!(self, Self::Float32 | Self::Float64)
    }

    pub(crate) fn data_type(self) -> DataType {
        match self {
            Self::Int8 => DataType::Int8,
            Self::Int16 => DataType::Int16,
            Self::Int32 => DataType::Int32,
            Self::UInt8 => DataType::UInt8,
            Self::UInt16 => DataType::UInt16,
            Self::UInt32 => DataType::UInt32,
            Self::Float32 => DataType::Float32,
            Self::Float64 => DataType::Float64,
        }
    }

    fn range(self) -> (f64, f64) {
        match self {
            Self::Int8 => (i8::MIN as f64, i8::MAX as f64),
            Self::Int16 => (i16::MIN as f64, i16::MAX as f64),
            Self::Int32 => (i32::MIN as f64, i32::MAX as f64),
            Self::UInt8 => (0.0, u8::MAX as f64),
            Self::UInt16 => (0.0, u16::MAX as f64),
            Self::UInt32 => (0.0, u32::MAX as f64),
            Self::Float32 => (f32::MIN as f64, f32::MAX as f64),
            Self::Float64 => (f64::MIN, f64::MAX),
        }
    }

    /// `v` as stored: rounded half to even and clamped for integer types,
    /// rounded to the nearest `f32` for `Float32`. Missing values (NaN) stay
    /// missing.
    pub fn quantise(self, v: f64) -> f64 {
        if v.is_nan() {
            return v;
        }
        match self {
            Self::Float64 => v,
            Self::Float32 => v as f32 as f64,
            _ => {
                let (lo, hi) = self.range();
                v.round_ties_even().clamp(lo, hi)
            }
        }
    }
}

/// How a column is reduced over the leaves under a coarser cell. Missing
/// values (NaN) are skipped; a cell whose leaves are all missing gets a
/// missing value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reducer {
    /// The mean, weighted by the leaf weights when given.
    Mean,
    /// The sum (weights are not applied).
    Sum,
    Min,
    Max,
    /// The most frequent value, by total weight when weights are given; ties
    /// go to the smallest value. For categories.
    Mode,
    /// The value of the representative leaf: the leaf closest to the cell's
    /// (weighted) mean over all `RepChild` columns, each divided by its
    /// standard deviation ([`ColumnSpec::sd`]: given, or computed over the
    /// leaves). Every `RepChild` column takes its value from the same leaf,
    /// so a row keeps a real combination of values (an RGB colour, say)
    /// rather than a blend.
    RepChild,
}

impl Reducer {
    pub const ALL: [Reducer; 6] = [Self::Mean, Self::Sum, Self::Min, Self::Max, Self::Mode, Self::RepChild];

    pub fn parse(s: &str) -> Result<Self> {
        Self::ALL.into_iter().find(|r| r.as_str() == s).ok_or_else(|| {
            let names: Vec<&str> = Self::ALL.iter().map(|r| r.as_str()).collect();
            invalid(format!("unknown reducer {s:?}; expected one of {}", names.join(", ")))
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Mean => "mean",
            Self::Sum => "sum",
            Self::Min => "min",
            Self::Max => "max",
            Self::Mode => "mode",
            Self::RepChild => "rep_child",
        }
    }
}

/// Name, storage type and reducer of a value column.
#[derive(Clone, Debug, PartialEq)]
pub struct ColumnSpec {
    pub name: String,
    pub ty: ColumnType,
    pub reducer: Reducer,
    /// For a `RepChild` column, the standard deviation that scales it in the
    /// distance to a cell's mean. In the leaves, `None` computes it over
    /// the leaves (1 where that is 0 or undefined, as for a constant
    /// column), and a value fixes it: builds of separate parts of a region
    /// that share it choose a cell's representative from its own leaves
    /// alone. In a built pyramid, the value used. Always `None` for other
    /// reducers.
    pub sd: Option<f64>,
}

/// A value column of the leaves: one value per leaf, NaN where missing.
#[derive(Clone, Debug)]
pub struct Column {
    pub spec: ColumnSpec,
    pub values: Vec<f64>,
}

impl Column {
    pub fn new(name: impl Into<String>, ty: ColumnType, reducer: Reducer, values: Vec<f64>) -> Self {
        Self {
            spec: ColumnSpec {
                name: name.into(),
                ty,
                reducer,
                sd: None,
            },
            values,
        }
    }

    /// The column with its `RepChild` scale fixed at `sd` ([`ColumnSpec::sd`]).
    pub fn with_sd(mut self, sd: f64) -> Self {
        self.spec.sd = Some(sd);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantise_rounds_half_to_even_and_clamps() {
        assert_eq!(ColumnType::UInt8.quantise(100.5), 100.0);
        assert_eq!(ColumnType::UInt8.quantise(101.5), 102.0);
        assert_eq!(ColumnType::UInt8.quantise(300.0), 255.0);
        assert_eq!(ColumnType::UInt8.quantise(-3.0), 0.0);
        assert_eq!(ColumnType::Int8.quantise(-200.0), -128.0);
        assert!(ColumnType::Int32.quantise(f64::NAN).is_nan());
        assert_eq!(ColumnType::Float32.quantise(0.1), 0.1f32 as f64);
        assert_eq!(ColumnType::Float64.quantise(0.1), 0.1);
    }

    #[test]
    fn names_round_trip() {
        for t in ColumnType::ALL {
            assert_eq!(ColumnType::parse(t.as_str()).unwrap(), t);
        }
        for r in Reducer::ALL {
            assert_eq!(Reducer::parse(r.as_str()).unwrap(), r);
        }
        assert!(ColumnType::parse("int64").is_err());
        assert!(Reducer::parse("median").is_err());
    }
}
