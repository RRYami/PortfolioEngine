use std::fmt;

#[derive(Debug)]
pub enum SurfaceError {
    Io(String, std::io::Error),
    Parquet(parquet::errors::ParquetError),
    Arrow(arrow::error::ArrowError),
    MissingColumn(String),
    BadColumn(&'static str),
}

impl fmt::Display for SurfaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(p, e) => write!(f, "{p}: {e}"),
            Self::Parquet(e) => write!(f, "parquet: {e}"),
            Self::Arrow(e) => write!(f, "arrow: {e}"),
            Self::MissingColumn(c) => write!(f, "options.parquet has no `{c}` column"),
            Self::BadColumn(c) => write!(f, "column `{c}` has an unexpected arrow type"),
        }
    }
}

impl std::error::Error for SurfaceError {}

impl From<parquet::errors::ParquetError> for SurfaceError {
    fn from(e: parquet::errors::ParquetError) -> Self {
        Self::Parquet(e)
    }
}

impl From<arrow::error::ArrowError> for SurfaceError {
    fn from(e: arrow::error::ArrowError) -> Self {
        Self::Arrow(e)
    }
}
