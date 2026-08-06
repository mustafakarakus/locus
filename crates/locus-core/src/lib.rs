//! Core library for Locus: shared error types, logging setup, and the building
//! blocks that the CLI, daemon, and MCP server depend on.
//!
//! This crate is scaffolding at U-001; storage, search, and the context brief
//! engine are implemented in later use cases (U-002+).

pub mod context;
pub mod error;
pub mod logging;
pub mod memory;
pub mod search;
pub mod store;

pub use error::{Error, Result};

/// Crate version, sourced from Cargo at build time.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_not_empty() {
        assert!(!VERSION.is_empty());
    }
}
