//! Shared testing utilities for Locus.
//!
//! Scaffolding at U-001. Fixtures, the dataset generator (U-012), an MCP test
//! client, and benchmark helpers are added alongside the use cases that need
//! them.

pub mod dataset;
pub mod stats;

/// Returns the core crate version, used by smoke tests to confirm the testkit
/// links against `locus-core`.
pub fn core_version() -> &'static str {
    locus_core::VERSION
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links_against_core() {
        assert!(!core_version().is_empty());
    }
}
