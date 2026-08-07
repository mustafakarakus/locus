## DECISIONS (locked before U-003 / U-006)

### D-1 — Search engine
SQLite FTS5 by default, behind a minimal `SearchEngine` trait.
Tantivy is a benchmark-gated upgrade, not the starting point.
- The trait models Locus's query needs (search / upsert / remove), not engine features.
- Engines return relevance-ranked candidates only.
- Recency + importance re-ranking lives in a shared layer above the engine.
- No `commit()`/`refresh()` in the trait; engines manage their own durability.
- Benchmark must specifically test code-identifier, partial-name, and typo queries.

### D-2 — Concurrency
`std::thread` + blocking I/O + single-writer channel.
No async runtime in v1. Revisit only if concurrency requirements grow.

### D-3 — IPC transport
Use the `interprocess` crate for Unix socket + named pipe.
Do not hand-roll the transport abstraction.

### D-4 — Secret handling
Vendor gitleaks/detect-secrets rule sets as static data.
Redact-or-warn by default: never silently drop, never hard-reject without flagging.
- Warnings are first-class (see D-6).
- License/attribution of vendored files is a scope checklist item, not a caveat.

### D-5 — Automaticity
MCP is pull-based and best-effort.
Add a first-class Hook-Based Context Injection use case.
- Host-specific adapters (Claude Code first), not one generic hooks API.
- Shared `ContextBrief` generation path with MCP (different trigger, same code path).
- Explicit default-query strategy for pre-reasoning injection.

### D-6 — IPC response envelope
Add `warnings` array alongside `ok` / `payload`.
- Warnings are non-fatal (`ok = true`).
- Errors are fatal (`ok = false`).
- MCP tool results must propagate warnings so agents can surface them.
- Cap and dedupe warnings (max 5).

### D-7 — U-003 tokenizer strategy
For the default FTS5 backend, use `unicode61` with identifier-friendly
token chars and prefix indexes, plus a substring fallback query path for
partial-name matching.
- This preserves phrase/prefix behavior while still covering practical partial
  identifier lookups.
- Trigram tokenizer behavior is benchmarked/measured as part of query suites,
  and can be revisited in U-012 if evidence shows a better trade-off.

### D-8 — U-006 daemon architecture
Decisions made while implementing `locusd` (U-006). None change the locked
search engine, storage engine, or IPC transport from D-1/D-2/D-3.

- **Health & stale-state detection is connect+ping, not PID liveness.**
  The daemon and clients decide "is another daemon alive?" by attempting an
  IPC `ping`, not by probing a PID with `kill(0)` / `OpenProcess`. This keeps
  the crate `unsafe`-free (`unsafe_code = "forbid"`) and is transport-honest:
  the thing that matters is whether the endpoint answers, not whether some PID
  exists. A stale socket file (no answer) is removed and re-bound; a live
  endpoint (answers) blocks a second start. The PID file is written for
  operator visibility only, not used as the liveness authority.

- **Detached start is "CLI spawns `locusd --foreground` with null stdio",
  not a Unix double-fork daemonize.** True daemonization needs `fork`/`setsid`
  (unsafe FFI), which the forbid-unsafe rule disallows. Auto-start therefore
  launches the foreground daemon as a child with detached, null stdio and
  polls the endpoint until it answers. Good enough for CLI/MCP/hook auto-start
  and identical across platforms.

- **Socket file permissions are tightened with a post-create `chmod`, not
  `ListenerOptionsExt::mode`.** `interprocess`'s `mode()` returns
  `ErrorKind::Unsupported` on macOS, so `create_listener` sets `0600` on the
  socket file after binding (defence in depth on top of the `0700` data dir).

- **Signal handling uses `ctrlc` (termination feature) for SIGINT+SIGTERM.**
  Avoids a hand-rolled unsafe `sigaction` handler while still giving clean,
  drain-then-exit shutdown.

- **Idle wait is a `Condvar` timed wait, and shutdown wakes the blocking
  accept loop by self-dialing the endpoint.** This gives near-zero idle CPU
  (no polling) while keeping the accept loop on simple blocking I/O per D-2
  (no async runtime).

- **Warm state today = warm process + OS page cache + established WAL + mmap;
  per-request SQLite handles are still opened lightweight. A persistent
  connection pool with cached prepared statements is deferred.** The daemon
  removes the dominant cost the use case targets — repeated cold *process*
  startup (Rust binary load + dynamic linking) — and keeps the database file
  hot. Fully persisting the `Connection` and switching every query to
  `prepare_cached` requires threading a shared `&Connection` through the core
  `Store` and `Fts5SearchEngine` query APIs (a U-003/U-004 hot-path change with
  single-writer and namespace-isolation implications). That optimization is
  deferred rather than rushed; it is a performance refinement, not a behavior
  change, and can land without altering the IPC protocol. Flagged in U-006 as
  a known gap for the reviewer.
