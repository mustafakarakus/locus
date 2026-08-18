//! Single-writer thread that serializes all mutating store operations.
//!
//! Reads (search, context, status, ping) run directly on the caller's handler
//! thread against read-only connections. Every write goes through this one
//! channel, giving Locus a single-writer path to SQLite (see DECISIONS D-2).
//!
//! Side-effect events for the live viewer (U-016) are published from this
//! thread on writes; search/context access events are published by the handler
//! that owns the retrieved memories. Both paths use the non-blocking
//! [`EventBus`] so a slow live client can never stall the writer.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use locus_core::capture::{capture, CaptureOutcome, CaptureTrigger};
use locus_core::events::EventBus;
use locus_core::ipc::protocol::{MemoryEvent, MemoryEventKind, Warning};
use locus_core::memory::NewMemory;
use locus_core::store::Store;
use locus_core::Result;

/// A mutating operation to run on the writer thread.
pub enum WriterOp {
    Remember(NewMemory, bool),
    Forget(String),
    Reindex,
    /// Best-effort access tracking bump (U-016); never read on the response
    /// path. Failures are dropped silently.
    RecordAccess(Vec<String>),
    /// Capture a host compaction summary into typed memories (U-017).
    Capture(CaptureTrigger),
    /// Drain marker: the writer finishes every already-queued op, then exits.
    Shutdown,
}

/// The result of a successful writer operation.
pub enum WriterOk {
    Remembered(String, Vec<Warning>),
    Forgotten,
    Reindexed(usize),
    Recorded,
    Captured(CaptureOutcome),
}

struct WriterJob {
    op: WriterOp,
    reply: Option<Sender<Result<WriterOk>>>,
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
            reply: Some(reply_tx),
        };
        self.send(job)?;
        reply_rx
            .recv()
            .map_err(|_| locus_core::Error::Other("writer thread dropped the reply".to_string()))?
    }

    /// Submits a fire-and-forget operation without waiting for a result.
    ///
    /// The send itself never blocks; if the writer thread has already exited the
    /// operation is dropped.
    pub fn submit_async(&self, op: WriterOp) {
        let job = WriterJob { op, reply: None };
        let _ = self.tx.send(job);
    }

    fn send(&self, job: WriterJob) -> Result<()> {
        if self.tx.send(job).is_err() {
            return Err(locus_core::Error::Other(
                "writer thread is unavailable".to_string(),
            ));
        }
        Ok(())
    }
}

/// Spawns the writer thread and returns its handle plus join handle.
///
/// The thread exits cleanly when every [`WriterHandle`] is dropped.
pub fn spawn(store: Store, events: EventBus) -> (WriterHandle, Option<JoinHandle<()>>) {
    let (tx, rx) = mpsc::channel::<WriterJob>();
    let join = match std::thread::Builder::new()
        .name("locusd-writer".to_string())
        .spawn(move || writer_loop(store, rx, events))
    {
        Ok(join) => Some(join),
        // A failed spawn must not abort the whole daemon (release builds use
        // panic = "abort"); the store simply runs shared-single-writer-free.
        Err(err) => {
            tracing::error!(target: "locusd", "failed to spawn writer thread: {err}");
            None
        }
    };
    (WriterHandle { tx }, join)
}

fn writer_loop(store: Store, rx: Receiver<WriterJob>, events: EventBus) {
    while let Ok(job) = rx.recv() {
        if matches!(job.op, WriterOp::Shutdown) {
            // Drain marker: every job queued before this one has already been
            // received (recv is FIFO); nothing may be enqueued after shutdown.
            break;
        }
        let result = run_op(&store, &events, job.op);
        // The receiver may have gone away if the client disconnected; ignore.
        if let Some(reply) = job.reply {
            let _ = reply.send(result);
        }
    }
}

fn run_op(store: &Store, events: &EventBus, op: WriterOp) -> Result<WriterOk> {
    match op {
        WriterOp::Remember(new_memory, allow_secret) => {
            let (id, warnings) = store.insert_memory_checked(new_memory, allow_secret)?;
            // Best-effort conflict detection: a failure here must not lose the
            // canonical memory that was just inserted.
            if let Ok(memory) = store.get_memory_by_id(&id) {
                let _ = store.detect_and_store_conflicts(&memory);
                events.publish(&memory_event(MemoryEventKind::Created, &memory, 0));
            }
            Ok(WriterOk::Remembered(id, warnings))
        }
        WriterOp::Forget(id) => {
            store.delete_memory(&id)?;
            Ok(WriterOk::Forgotten)
        }
        WriterOp::Reindex => {
            let count = store.reindex()?;
            Ok(WriterOk::Reindexed(count))
        }
        WriterOp::RecordAccess(ids) => {
            let _ = store.record_access(&ids);
            Ok(WriterOk::Recorded)
        }
        WriterOp::Capture(trigger) => {
            let outcome = capture(store, &trigger)?;
            for id in &outcome.written_ids {
                if let Ok(memory) = store.get_memory_by_id(id) {
                    events.publish(&memory_event(MemoryEventKind::Created, &memory, 0));
                }
            }
            Ok(WriterOk::Captured(outcome))
        }
        WriterOp::Shutdown => {
            // Unreachable in practice (writer_loop breaks on the marker before
            // dispatching), but keep the match exhaustive.
            Ok(WriterOk::Recorded)
        }
    }
}

/// Builds a live event from a memory for a given kind and access delta.
pub fn memory_event(
    kind: MemoryEventKind,
    memory: &locus_core::memory::Memory,
    access_delta: u64,
) -> MemoryEvent {
    MemoryEvent {
        kind,
        memory_id: memory.id.clone(),
        title: memory.title.clone(),
        namespace: memory.namespace.clone(),
        memory_type: memory.memory_type.as_str().to_string(),
        importance: memory.importance,
        access_delta,
        timestamp: memory.updated_at,
    }
}
