//! Logging setup for Locus.
//!
//! Logging is **disabled by default**. `tracing` emits nothing unless a
//! subscriber is installed, and Locus never installs one implicitly. Callers
//! (typically a binary's `main`) opt in explicitly via [`init`], which honors
//! the `LOCUS_LOG` environment variable and stays silent when it is unset.

use std::sync::Once;

static INIT: Once = Once::new();

/// Environment variable that controls Locus log verbosity (e.g. `info`,
/// `debug`). When unset or empty, logging stays disabled.
pub const LOG_ENV: &str = "LOCUS_LOG";

/// Opt in to logging based on the `LOCUS_LOG` environment variable.
///
/// This is a no-op when `LOCUS_LOG` is unset or empty, keeping Locus silent by
/// default. It is safe to call multiple times; only the first call has effect.
///
/// At U-001 this only records intent; a concrete subscriber is wired up when a
/// tracing backend is added in a later use case.
pub fn init() {
    INIT.call_once(|| {
        match std::env::var(LOG_ENV) {
            Ok(level) if !level.trim().is_empty() => {
                tracing::debug!(target: "locus", requested_level = %level, "logging enabled");
            }
            _ => {
                // Disabled by default: install nothing, emit nothing.
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_is_idempotent_and_silent_by_default() {
        // Should not panic regardless of environment state.
        init();
        init();
    }

    #[test]
    fn log_env_constant_is_stable() {
        assert_eq!(LOG_ENV, "LOCUS_LOG");
    }
}
