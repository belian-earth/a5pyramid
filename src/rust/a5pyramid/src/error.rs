use thiserror::Error;

/// Errors from building, writing and reading pyramids. The messages start
/// with a fixed prefix per variant, which bindings may use to classify them.
#[derive(Error, Debug)]
pub enum Error {
    /// Input that cannot make a pyramid: invalid or duplicated cells, mixed
    /// resolutions, mismatched column lengths, bad options.
    #[error("invalid input: {0}")]
    Invalid(String),

    /// A file that is not an a5pyramid file, or one in a format version this
    /// crate does not read.
    #[error("format error: {0}")]
    Format(String),

    /// An error from the a5 crate.
    #[error("a5 error: {0}")]
    A5(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("parquet error: {0}")]
    Parquet(#[from] parquet::errors::ParquetError),

    #[error("arrow error: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// The work was stopped through a [`crate::Cancel`] token.
    #[error("interrupted: stopped by the user")]
    Cancelled,
}

pub type Result<T> = std::result::Result<T, Error>;

pub(crate) fn invalid(msg: impl Into<String>) -> Error {
    Error::Invalid(msg.into())
}
