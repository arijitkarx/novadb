use std::fmt;

/// The top-level error type for NovaDB.
#[derive(Debug)]
pub enum NovaError {
    /// An I/O error (missing files, permissions, disk full, ...).
    Io(std::io::Error),
    /// A file exists but is corrupt: bad magic, version, or checksum.
    Corrupt(String),
    /// A collection with the given name does not exist.
    CollectionNotFound(String),
    /// A collection with the given name already exists.
    CollectionExists(String),
    /// A record with the given id does not exist.
    RecordNotFound(u64),
    /// A vector's dimensionality does not match the collection's.
    DimMismatch { expected: usize, got: usize },
    /// An empty vector was provided where one is required.
    EmptyVector,
    /// A metadata filter expression could not be parsed.
    Parse(String),
    /// Serialization/deserialization failed (bincode/serde).
    Encoding(String),
}

impl fmt::Display for NovaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NovaError::Io(e) => write!(f, "i/o error: {e}"),
            NovaError::Corrupt(msg) => write!(f, "corrupt file: {msg}"),
            NovaError::CollectionNotFound(name) => write!(f, "collection '{name}' not found"),
            NovaError::CollectionExists(name) => write!(f, "collection '{name}' already exists"),
            NovaError::RecordNotFound(id) => write!(f, "record {id} not found"),
            NovaError::DimMismatch { expected, got } => {
                write!(f, "dimension mismatch: expected {expected}, got {got}")
            }
            NovaError::EmptyVector => write!(f, "vector must not be empty"),
            NovaError::Parse(msg) => write!(f, "parse error: {msg}"),
            NovaError::Encoding(msg) => write!(f, "encoding error: {msg}"),
        }
    }
}

impl std::error::Error for NovaError {}

impl From<std::io::Error> for NovaError {
    fn from(e: std::io::Error) -> Self {
        NovaError::Io(e)
    }
}

impl From<bincode::Error> for NovaError {
    fn from(e: bincode::Error) -> Self {
        NovaError::Encoding(e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, NovaError>;
