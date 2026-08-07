//! Shared error handling for Locus.
//!
//! Library crates return [`Error`]; binaries may wrap these with `anyhow` for
//! ergonomic top-level handling.

use std::result::Result as StdResult;

/// Convenient alias used throughout Locus core APIs.
pub type Result<T> = StdResult<T, Error>;

/// The canonical error type for `locus-core`.
///
/// Variants are intentionally minimal at U-001 and will grow as storage,
/// search, and IPC land in later use cases.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// An operation received input that failed validation.
    #[error("invalid input: {0}")]
    InvalidInput(String),

    /// A requested item could not be found.
    #[error("not found: {0}")]
    NotFound(String),

    /// An underlying I/O operation failed.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// An underlying SQLite operation failed.
    #[error("database error: {0}")]
    Sql(#[from] rusqlite::Error),

    /// A serialization or deserialization operation failed.
    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    /// A catch-all for errors that do not yet have a dedicated variant.
    #[error("{0}")]
    Other(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_input_displays_message() {
        let err = Error::InvalidInput("bad namespace".to_string());
        assert_eq!(err.to_string(), "invalid input: bad namespace");
    }

    #[test]
    fn io_error_converts() {
        let io = std::io::Error::other("boom");
        let err: Error = io.into();
        assert!(matches!(err, Error::Io(_)));
    }

    #[test]
    fn sql_error_converts() {
        let err: Error = rusqlite::Error::InvalidQuery.into();
        assert!(matches!(err, Error::Sql(_)));
    }
}
