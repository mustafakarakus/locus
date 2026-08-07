//! Filesystem locations and the platform IPC endpoint for a Locus instance.
//!
//! Everything a running daemon touches lives under a single user-owned data
//! directory (default `~/.locus`, overridable with the `LOCUS_HOME`
//! environment variable, primarily for tests). The IPC endpoint is a Unix
//! domain socket on Unix platforms and a namespaced named pipe on Windows.

use std::io;
use std::path::{Path, PathBuf};

use interprocess::local_socket::{GenericFilePath, GenericNamespaced, Name, ToFsName, ToNsName};

use crate::{Error, Result};

/// Environment variable that overrides the Locus data directory.
pub const HOME_ENV: &str = "LOCUS_HOME";

const DIR_NAME: &str = ".locus";
const SOCKET_NAME: &str = "s.sock";
const LOCK_NAME: &str = "locus.lock";
const PID_NAME: &str = "locus.pid";
const LOG_DIR: &str = "logs";
const LOG_NAME: &str = "locusd.log";

/// A platform-appropriate IPC endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Endpoint {
    /// A filesystem-path Unix domain socket.
    Path(PathBuf),
    /// A namespaced Windows named pipe (or abstract namespace) identifier.
    Namespaced(String),
}

impl Endpoint {
    /// Human-readable transport name for status output.
    pub fn transport(&self) -> &'static str {
        match self {
            Endpoint::Path(_) => "unix-socket",
            Endpoint::Namespaced(_) => "named-pipe",
        }
    }

    /// Displayable endpoint identifier (socket path or pipe name).
    pub fn display(&self) -> String {
        match self {
            Endpoint::Path(path) => path.display().to_string(),
            Endpoint::Namespaced(name) => name.clone(),
        }
    }

    /// Converts the endpoint into an `interprocess` socket [`Name`].
    pub fn to_name(&self) -> io::Result<Name<'static>> {
        match self {
            Endpoint::Path(path) => path.clone().to_fs_name::<GenericFilePath>(),
            Endpoint::Namespaced(name) => name.clone().to_ns_name::<GenericNamespaced>(),
        }
    }

    /// Returns the on-disk socket file path, if this endpoint is filesystem
    /// based. Used for stale-socket cleanup.
    pub fn socket_file(&self) -> Option<&Path> {
        match self {
            Endpoint::Path(path) => Some(path.as_path()),
            Endpoint::Namespaced(_) => None,
        }
    }
}

/// Resolved filesystem locations for a Locus instance.
#[derive(Debug, Clone)]
pub struct Paths {
    data_dir: PathBuf,
    endpoint: Endpoint,
}

impl Paths {
    /// Resolves the default paths, honoring `LOCUS_HOME`.
    pub fn resolve() -> Result<Self> {
        let data_dir = default_data_dir()?;
        Ok(Self::from_data_dir(data_dir))
    }

    /// Builds paths rooted at an explicit data directory (used by tests).
    pub fn from_data_dir(data_dir: impl Into<PathBuf>) -> Self {
        let data_dir = data_dir.into();
        let endpoint = endpoint_for(&data_dir);
        Self { data_dir, endpoint }
    }

    /// The user-owned data directory that holds all Locus state.
    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// The IPC endpoint for this instance.
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// Path to the canonical SQLite database.
    pub fn db_file(&self) -> PathBuf {
        self.data_dir.join("locus.db")
    }

    /// Path to the daemon lock file.
    pub fn lock_file(&self) -> PathBuf {
        self.data_dir.join(LOCK_NAME)
    }

    /// Path to the daemon PID file.
    pub fn pid_file(&self) -> PathBuf {
        self.data_dir.join(PID_NAME)
    }

    /// Path to the daemon log file.
    pub fn log_file(&self) -> PathBuf {
        self.data_dir.join(LOG_DIR).join(LOG_NAME)
    }

    /// Creates the data directory (and log subdirectory) with restrictive
    /// permissions where the platform supports it.
    pub fn ensure_dirs(&self) -> Result<()> {
        ensure_private_dir(&self.data_dir)?;
        ensure_private_dir(&self.data_dir.join(LOG_DIR))?;
        Ok(())
    }
}

fn default_data_dir() -> Result<PathBuf> {
    if let Ok(custom) = std::env::var(HOME_ENV) {
        if !custom.trim().is_empty() {
            return Ok(PathBuf::from(custom));
        }
    }

    let home = home_dir().ok_or_else(|| {
        Error::Other("could not determine home directory for Locus data".to_string())
    })?;
    Ok(home.join(DIR_NAME))
}

#[cfg(windows)]
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE").map(PathBuf::from)
}

#[cfg(not(windows))]
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

#[cfg(windows)]
fn endpoint_for(data_dir: &Path) -> Endpoint {
    // Named pipes are keyed by name, not path. Derive a stable per-directory
    // identifier so multiple data dirs (e.g. tests) don't collide.
    let user = std::env::var("USERNAME").unwrap_or_else(|_| "user".to_string());
    let key = short_key(data_dir);
    Endpoint::Namespaced(format!("locus-{user}-{key}.sock"))
}

#[cfg(not(windows))]
fn endpoint_for(data_dir: &Path) -> Endpoint {
    // On Linux, prefer the runtime dir for the default location to keep the
    // socket path short and on a tmpfs. A custom data dir (tests) always keeps
    // the socket beside the database for isolation.
    if is_default_dir(data_dir) {
        if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
            let runtime = PathBuf::from(runtime);
            if !runtime.as_os_str().is_empty() {
                return Endpoint::Path(runtime.join("locus").join("locus.sock"));
            }
        }
    }
    Endpoint::Path(data_dir.join(SOCKET_NAME))
}

#[cfg(not(windows))]
fn is_default_dir(data_dir: &Path) -> bool {
    default_data_dir()
        .map(|default| default == data_dir)
        .unwrap_or(false)
}

#[cfg(windows)]
fn short_key(path: &Path) -> String {
    // Simple, dependency-free stable hash of the path string.
    let mut hash: u64 = 1469598103934665603;
    for byte in path.to_string_lossy().as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(1099511628211);
    }
    format!("{hash:016x}")
}

#[cfg(unix)]
fn ensure_private_dir(dir: &Path) -> Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    if !dir.exists() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
    } else {
        let mut perms = std::fs::metadata(dir)?.permissions();
        perms.set_mode(0o700);
        std::fs::set_permissions(dir, perms)?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn ensure_private_dir(dir: &Path) -> Result<()> {
    if !dir.exists() {
        std::fs::create_dir_all(dir)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_rooted_at_data_dir() {
        let paths = Paths::from_data_dir("/tmp/locus-test");
        assert_eq!(paths.db_file(), PathBuf::from("/tmp/locus-test/locus.db"));
        assert_eq!(
            paths.lock_file(),
            PathBuf::from("/tmp/locus-test/locus.lock")
        );
        assert_eq!(paths.pid_file(), PathBuf::from("/tmp/locus-test/locus.pid"));
        assert!(paths.log_file().ends_with("logs/locusd.log"));
    }

    #[cfg(not(windows))]
    #[test]
    fn custom_dir_keeps_socket_beside_db() {
        let paths = Paths::from_data_dir("/tmp/locus-test");
        match paths.endpoint() {
            Endpoint::Path(path) => assert_eq!(path, &PathBuf::from("/tmp/locus-test/s.sock")),
            other => panic!("expected path endpoint, got {other:?}"),
        }
        assert_eq!(paths.endpoint().transport(), "unix-socket");
    }
}
