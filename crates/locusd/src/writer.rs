//! Single-writer thread that serializes all mutating store operations.
//!
//! Reads (search, context, status, ping) run directly on the caller's handler
//! thread against read-only connections. Every write goes through this one
//! channel, giving Locus a single-writer path to SQLite (see DECISIONS D-2).

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use locus_core::memory::NewMemory;
use locus_core::store::Store;
use locus_core::Result;

/// A mutating operation to run on the writer thread.
pub enum WriterOp {
    Remember(NewMemory),
    Forget(String),
    Reindex,
}

/// The result of a successful writer operation.
pub enum WriterOk {
    Remembered(String),
    Forgotten,
    Reindexed(usize),
}

struct WriterJob {
    op: WriterOp,
    reply: Sender<Result<WriterOk>>,
}

/// Handle used to submit jobs to the writer thread.
#[derive(Clone)]
pub struct WriterHandle {
    tx: Sender<WriterJob>,
}

impl WriterHandle {
    /// Submits a write and blocks until the writer thread reports a result.
    pub fn submit(&self, op: WriterOp) -> Result<WriterOk> {
        let (reply_tx, reply_rx) = mpsc::channel();
        let job = WriterJob {
            op,
            reply: reply_tx,
        };
        if self.tx.send(job).is_err() {
            return Err(locus_core::Error::Other(
                "writer thread is unavailable".to_string(),
            ));
        }
        reply_rx
            .recv()
            .map_err(|_| locus_core::Error::Other("writer thread dropped the reply".to_string()))?
    }
}

/// Spawns the writer thread and returns its handle plus join handle.
///
/// The thread exits cleanly when every [`WriterHandle`] is dropped.
pub fn spawn(store: Store) -> (WriterHandle, JoinHandle<()>) {
    let (tx, rx) = mpsc::channel::<WriterJob>();
    let join = std::thread::Builder::new()
        .name("locusd-writer".to_string())
        .spawn(move || writer_loop(store, rx))
        .expect("failed to spawn writer thread");
    (WriterHandle { tx }, join)
}

fn writer_loop(store: Store, rx: Receiver<WriterJob>) {
    while let Ok(job) = rx.recv() {
        let result = run_op(&store, job.op);
        // The receiver may have gone away if the client disconnected; ignore.
        let _ = job.reply.send(result);
    }
}

fn run_op(store: &Store, op: WriterOp) -> Result<WriterOk> {
    match op {
        WriterOp::Remember(new_memory) => {
            let id = store.insert_memory(new_memory)?;
            Ok(WriterOk::Remembered(id))
        }
        WriterOp::Forget(id) => {
            store.delete_memory(&id)?;
            Ok(WriterOk::Forgotten)
        }
        WriterOp::Reindex => {
            let count = store.reindex()?;
            Ok(WriterOk::Reindexed(count))
        }
    }
}
