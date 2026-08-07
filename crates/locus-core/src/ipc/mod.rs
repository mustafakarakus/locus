//! Local, versioned IPC layer shared by the daemon and its clients.
//!
//! - [`protocol`] defines the newline-delimited JSON request/response envelope.
//! - [`paths`] resolves the platform endpoint and on-disk state locations.
//! - [`wire`] frames messages on the byte stream.
//! - [`client`] is a thin request helper with daemon auto-start.

pub mod client;
pub mod paths;
pub mod protocol;
pub mod wire;

pub use client::DaemonClient;
pub use paths::{Endpoint, Paths};
pub use protocol::{Request, Response, ResponseError, Warning};
