//! Daemon configuration derived from command-line flags.

use std::path::PathBuf;
use std::time::Duration;

/// Suggested default idle timeout before the daemon self-exits.
pub const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 600;

/// Runtime configuration for a `locusd` instance.
#[derive(Debug, Clone)]
pub struct Config {
    /// Explicit data directory, if provided. `None` resolves the default.
    pub data_dir: Option<PathBuf>,
    /// Idle timeout after which the daemon exits when no requests arrive.
    pub idle_timeout: Duration,
    /// When true, the daemon never self-exits on idle.
    pub no_idle_exit: bool,
    /// Requested log level (`off`, `error`, `warn`, `info`, `debug`).
    pub log_level: LogLevel,
    /// Whether the daemon was started in explicit foreground mode.
    pub foreground: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            data_dir: None,
            idle_timeout: Duration::from_secs(DEFAULT_IDLE_TIMEOUT_SECS),
            no_idle_exit: false,
            log_level: LogLevel::Warn,
            foreground: false,
        }
    }
}

/// Minimal, dependency-free log level.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Off,
    Error,
    Warn,
    Info,
    Debug,
}

impl LogLevel {
    /// Parses a level string, defaulting to `Warn` on unknown input.
    pub fn parse(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "off" | "none" | "silent" => Self::Off,
            "error" => Self::Error,
            "warn" | "warning" => Self::Warn,
            "info" => Self::Info,
            "debug" | "trace" => Self::Debug,
            _ => Self::Warn,
        }
    }

    /// Short label used in log lines.
    pub fn label(self) -> &'static str {
        match self {
            Self::Off => "OFF",
            Self::Error => "ERROR",
            Self::Warn => "WARN",
            Self::Info => "INFO",
            Self::Debug => "DEBUG",
        }
    }
}
