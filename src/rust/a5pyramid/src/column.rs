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

    /// Whether a vector column can hold this type: one byte an element.
    pub fn is_byte(self) -> bool {
        matches!(self, Self::Int8 | Self::UInt8)
    }

    /// A vector element's value from its byte (`Int8` or `UInt8`).
    #[inline]
    pub(crate) fn byte_value(self, b: u8) -> f64 {
        match self {
            Self::Int8 => b as i8 as f64,
            _ => b as f64,
        }
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
    /// The mean, weighted by the leaf weights when given; of the decoded
    /// values for a column with a [`Codec`], encoded again.
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

/// How a column's stored codes map to the values they stand for. A `Mean`
/// over a column with a codec decodes each leaf's code, takes the
/// (weighted) mean of the values and encodes it, so that a coarse cell
/// holds the code of the mean of its leaves' values. A nonlinear codec does
/// not commute with the mean: the mean of the codes is not the code of the
/// mean.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Codec {
    /// AlphaEarth Foundations (AEF) embeddings: `int8` codes, decoded as
    /// `sign(x) * (x / 127.5)^2` and encoded as
    /// `round(sign(v) * sqrt(|v|) * 127.5)`, rounded half away from zero and
    /// clamped to -128 to 127, the encoding of the a5embeddings store.
    Aef,
}

impl Codec {
    pub const ALL: [Codec; 1] = [Self::Aef];

    pub fn parse(s: &str) -> Result<Self> {
        Self::ALL.into_iter().find(|c| c.as_str() == s).ok_or_else(|| {
            let names: Vec<&str> = Self::ALL.iter().map(|c| c.as_str()).collect();
            invalid(format!("unknown codec {s:?}; expected one of {}", names.join(", ")))
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Aef => "aef",
        }
    }

    /// The storage type of the codes.
    pub fn code_type(self) -> ColumnType {
        match self {
            Self::Aef => ColumnType::Int8,
        }
    }

    /// The value a code stands for. Missing codes (NaN) stay missing.
    #[inline]
    pub fn decode(self, code: f64) -> f64 {
        match self {
            Self::Aef => {
                let t = code / 127.5;
                t * t.abs()
            }
        }
    }

    /// The code of a value. Missing values (NaN) stay missing.
    #[inline]
    pub fn encode(self, value: f64) -> f64 {
        if value.is_nan() {
            return value;
        }
        match self {
            // f64::round rounds half away from zero, as DuckDB's ROUND in
            // the store's encoder
            Self::Aef => (value.signum() * value.abs().sqrt() * 127.5)
                .round()
                .clamp(-128.0, 127.0),
        }
    }

    /// The value of every byte, for a vector column of `ty` elements.
    pub(crate) fn byte_values(self, ty: ColumnType) -> [f64; 256] {
        let mut lut = [0.0; 256];
        for (b, v) in lut.iter_mut().enumerate() {
            *v = self.decode(ty.byte_value(b as u8));
        }
        lut
    }
}

/// Name, storage type and reducer of a value column.
#[derive(Clone, Debug, PartialEq)]
pub struct ColumnSpec {
    pub name: String,
    pub ty: ColumnType,
    pub reducer: Reducer,
    /// How the stored codes map to values ([`Codec`]), for a `Mean` column
    /// of the codec's code type; `None` stores the values themselves.
    pub codec: Option<Codec>,
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
                codec: None,
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

    /// The column with its codes decoded by `codec` ([`ColumnSpec::codec`]).
    pub fn with_codec(mut self, codec: Codec) -> Self {
        self.spec.codec = Some(codec);
        self
    }
}

/// Name, element type, length, reducer and `RepChild` scale of a vector
/// column: a fixed number of 8-bit values per cell, such as an embedding,
/// stored as one fixed-length binary value per row. A vector column is
/// reduced by `RepChild`, together with every other `RepChild` column, or
/// by `Mean`, element by element.
#[derive(Clone, Debug, PartialEq)]
pub struct VectorSpec {
    pub name: String,
    /// `Int8` or `UInt8`.
    pub ty: ColumnType,
    /// Elements per cell.
    pub dim: usize,
    /// `RepChild` (the default) or `Mean`.
    pub reducer: Reducer,
    /// How each element's code maps to its value, for a `Mean` vector of
    /// the codec's code type, as [`ColumnSpec::codec`].
    pub codec: Option<Codec>,
    /// For a `RepChild` vector, one standard deviation for every element,
    /// scaling the vector in the distance to a cell's mean, as
    /// [`ColumnSpec::sd`]: `None` computes it over all the leaves' elements
    /// (1 where that is 0 or undefined), a value fixes it. In a built
    /// pyramid, the value used. Always `None` for a `Mean` vector.
    pub sd: Option<f64>,
}

/// A vector column of the leaves: `dim` elements per leaf, row by row, as
/// bytes (an `Int8` element is its two's complement byte). Vectors are
/// never missing.
#[derive(Clone, Debug)]
pub struct VectorColumn {
    pub spec: VectorSpec,
    pub values: Vec<u8>,
}

impl VectorColumn {
    pub fn new(name: impl Into<String>, ty: ColumnType, dim: usize, values: Vec<u8>) -> Self {
        Self {
            spec: VectorSpec {
                name: name.into(),
                ty,
                dim,
                reducer: Reducer::RepChild,
                codec: None,
                sd: None,
            },
            values,
        }
    }

    /// The column with its `RepChild` scale fixed at `sd`.
    pub fn with_sd(mut self, sd: f64) -> Self {
        self.spec.sd = Some(sd);
        self
    }

    /// The column reduced by `reducer` (`RepChild` or `Mean`).
    pub fn with_reducer(mut self, reducer: Reducer) -> Self {
        self.spec.reducer = reducer;
        self
    }

    /// The column with its elements decoded by `codec` in a `Mean`.
    pub fn with_codec(mut self, codec: Codec) -> Self {
        self.spec.codec = Some(codec);
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
        for c in Codec::ALL {
            assert_eq!(Codec::parse(c.as_str()).unwrap(), c);
        }
        assert!(ColumnType::parse("int64").is_err());
        assert!(Reducer::parse("median").is_err());
        assert!(Codec::parse("sqrt").is_err());
    }

    #[test]
    fn aef_codes_round_trip_and_match_the_store_encoder() {
        let aef = Codec::Aef;
        for q in -128..=127 {
            let q = q as f64;
            assert_eq!(aef.encode(aef.decode(q)), q, "code {q}");
        }
        assert_eq!(aef.decode(127.0), (127.0f64 / 127.5).powi(2));
        assert_eq!(aef.decode(-64.0), -(64.0f64 / 127.5).powi(2));
        assert_eq!(aef.decode(0.0), 0.0);
        // the store's SQL: ROUND(SIGN(v) * SQRT(ABS(v)) * 127.5), half away
        // from zero, clamped to -128..127
        let store = |v: f64| (v.signum() * v.abs().sqrt() * 127.5).round().clamp(-128.0, 127.0);
        for k in -2000..=2000 {
            let v = k as f64 / 1990.0;
            assert_eq!(aef.encode(v), store(v), "value {v}");
        }
        // just past a half step, either side of zero
        assert_eq!(aef.encode((10.5f64 / 127.5).powi(2) * (1.0 + 1e-12)), 11.0);
        assert_eq!(aef.encode(-(10.5f64 / 127.5).powi(2) * (1.0 + 1e-12)), -11.0);
        assert_eq!(aef.encode(2.0), 127.0);
        assert_eq!(aef.encode(-2.0), -128.0);
        assert!(aef.encode(f64::NAN).is_nan());
        assert!(aef.decode(f64::NAN).is_nan());
        assert_eq!(aef.code_type(), ColumnType::Int8);
        let lut = aef.byte_values(ColumnType::Int8);
        assert_eq!(lut[0x80], aef.decode(-128.0));
        assert_eq!(lut[0x7f], aef.decode(127.0));
    }
}
