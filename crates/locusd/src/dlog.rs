//! Tiny, dependency-free file logger for the daemon.
//!
//! Logs are metadata-only (never request payloads or memory content) and are
//! written to `<data_dir>/logs/locusd.log`. The file is truncated when it grows
//! past a small cap so idle daemons never accumulate large logs.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use time::OffsetDateTime;

use crate::config::LogLevel;

const MAX_LOG_BYTES: u64 = 1024 * 1024;

/// Thread-safe append-only logger with a size cap.
pub struct DaemonLog {
    level: LogLevel,
    path: PathBuf,
    file: Mutex<Option<File>>,
}

impl DaemonLog {
    /// Opens (or creates) the log file at `path`, filtering below `level`.
    pub fn open(path: &Path, level: LogLevel) -> Self {
        let file = OpenOptions::new().create(true).append(true).open(path).ok();

        #[cfg(unix)]
        if let Some(file) = &file {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(meta) = file.metadata() {
                let mut perms = meta.permissions();
                perms.set_mode(0o600);
                let _ = file.set_permissions(perms);
            }
        }

        Self {
            level,
            path: path.to_path_buf(),
            file: Mutex::new(file),
        }
    }

    /// Emits a log line if `level` is enabled.
    pub fn log(&self, level: LogLevel, message: &str) {
        if self.level == LogLevel::Off || level > self.level {
            return;
        }

        let now = OffsetDateTime::now_utc();
        let line = format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z [{}] {}\n",
            now.year(),
            u8::from(now.month()),
            now.day(),
            now.hour(),
            now.minute(),
            now.second(),
            level.label(),
            message,
        );

        let mut guard = match self.file.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };

        self.rotate_if_needed(&mut guard);

        if let Some(file) = guard.as_mut() {
            let _ = file.write_all(line.as_bytes());
            let _ = file.flush();
        }
    }

    /// Convenience helpers.
    pub fn info(&self, message: &str) {
        self.log(LogLevel::Info, message);
    }

    pub fn warn(&self, message: &str) {
        self.log(LogLevel::Warn, message);
    }

    pub fn error(&self, message: &str) {
        self.log(LogLevel::Error, message);
    }

    fn rotate_if_needed(&self, guard: &mut Option<File>) {
        let too_big = std::fs::metadata(&self.path)
            .map(|meta| meta.len() > MAX_LOG_BYTES)
            .unwrap_or(false);
        if !too_big {
            return;
        }

        // Truncate in place by reopening with truncation.
        if let Ok(file) = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&self.path)
        {
            *guard = OpenOptions::new().append(true).open(&self.path).ok();
            drop(file);
        }
    }
}
