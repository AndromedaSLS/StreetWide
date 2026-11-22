use std::fmt;

/// Errors from reading, writing or validating world-format data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Not valid JSON, or JSON that does not have the expected structure.
    Json(String),
    /// Valid JSON whose bytes are not the canonical serialisation.
    NotCanonical(String),
    /// A value breaks a rule of the world format (RFC 0001).
    Invalid(String),
    /// A glb file breaks the StreetWide glb profile.
    Glb(String),
    /// A file's bytes do not hash to the id it is stored under.
    HashMismatch { expected: String, actual: String },
    /// Filesystem error.
    Io(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Json(m) => write!(f, "invalid JSON: {m}"),
            Error::NotCanonical(m) => write!(f, "not canonical JSON: {m}"),
            Error::Invalid(m) => write!(f, "invalid: {m}"),
            Error::Glb(m) => write!(f, "invalid glb: {m}"),
            Error::HashMismatch { expected, actual } => {
                write!(f, "hash mismatch: expected {expected}, got {actual}")
            }
            Error::Io(m) => write!(f, "I/O error: {m}"),
        }
    }
}

impl std::error::Error for Error {}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Json(e.to_string())
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e.to_string())
    }
}
