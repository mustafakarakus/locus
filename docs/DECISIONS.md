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

### D-9 — U-007 MCP server is hand-rolled JSON-RPC over stdio
Official Rust MCP SDKs (`rmcp` and similar) pull in Tokio. That conflicts with
D-2 (blocking threads, no async runtime in v1). U-007 therefore implements a
minimal tools-only MCP server:

- newline-delimited JSON-RPC 2.0 on stdio (MCP transport rules)
- lifecycle: `initialize` → `notifications/initialized` → `tools/list` /
  `tools/call` / `ping`
- protocol versions `2025-03-26` and `2024-11-05`
- all tools talk to `locusd` through the existing IPC client (with auto-start)
- `memory_search` returns a `ContextBrief` via the daemon `context` command
  (same generator as CLI/hooks — D-5)
- IPC `warnings` are appended as a second text content block containing JSON
  `{"warnings":[...]}` so agents can surface them (D-6)

Stdout is MCP-only; logs go to stderr. No network, no REST.

### D-10 — U-016 memory visualization channel
Decisions made while scoping `locus graph` (U-016). This is the first
documented, explicitly-approved exception to "no REST / no network by
default": an opt-in, loopback-only HTTP listener used to *visualize* memory.
It is not a memory-serving API — it never exposes stored content to anything
but a local browser tab, never accepts writes, and only exists while a viz
client is connected.

- **Viz is a separate process (`locus-viz`), not a `locusd` feature.**
  `locus graph` (snapshot) writes a self-contained HTML file — data embedded,
  JS/CSS inlined, no CDN, works offline, zero network. `locus graph --live`
  spawns `locus-viz`, which serves that page over loopback HTTP (127.0.0.1
  only) with SSE push, subscribes to daemon events over the existing IPC
  transport, binds only while a client is connected, and exits when the tab
  closes. `locusd` gains no HTTP surface and no idle RSS from the listener.

- **Graph reads must never block or deadlock the memory path.** Graph queries
  run on their own read-only SQLite connections, never on the shared warm
  search connection and never through the single-writer queue. WAL snapshot
  isolation makes readers non-blocking against the single writer, so a graph
  query structurally cannot stall a `memory_save` or vice versa.

- **Live events are fire-and-forget with a bounded, drop-on-backpressure
  channel.** The daemon emits `memory_created` / `memory_searched` events on
  retrieval/save. A slow or hung viz client causes events to be dropped, never
  queued without bound — event emission can never stall `locusd` request
  handling. This is the one place an unbounded queue *could* have become a
  blocking hazard, so it is explicitly bounded.

- **Access tracking is cheap and decoupled.** `access_count` /
  `last_accessed_at` are bumped on the retrieval path as a non-blocking,
  batched/fire-and-forget update — never in the latency-critical search
  response path. "Most visited" is derived from these counters.

- **Rendering never re-routes secrets.** Viz output relies on U-011 redaction
  at write time; the viz path adds no new formatting or secret-handling path.

### D-11 — U-015 hook adapter approach
Decisions made while implementing Hook-Based Context Injection. Extends D-5:
the "host-specific adapters + shared brief path" strategy is now concrete.

- **Adapters are pure translators; injection is one shared function.**
  Each `HookAdapter` maps a host payload to a normalized `InjectTrigger`
  (`namespace`, optional `query`, `DefaultQueryStrategy`, `token_budget`).
  The single internal call `hooks::inject_context` then produces the brief
  through `store.context_brief` (exact MCP path) or `store.summary_brief`
  (session-start default-query path). There is exactly one formatting /
  compression code path — `context::build_context_brief` — for every trigger.

- **`locus hook context` is a stdin-payload reader with graceful failure.**
  It reads the host's hook JSON from stdin, translates it through the chosen
  adapter (`--host claude-code` default), and prints the brief to stdout (where
  the host captures it). On any failure — unparseable payload, unknown host,
  store error — it prints the diagnostic to stderr, emits `NO_RELEVANT_MEMORY`
  on stdout, and exits 0, so a hook failure can never block a host lifecycle
  event (Claude Code treats non-zero exits from some hooks as event-blocking).

- **Default-query strategy defaults to a namespace-scoped summary.**
  With no query (session-start), injection renders the namespace's recent
  memories through `build_context_brief` (Decisions category sorts first) under
  a 200-token budget. Strategy is selectable per invocation via
  `--strategy summary|none` — a project picks its own setting in its hook
  config, which is how per-namespace configurability is realized without a new
  config-file subsystem. `none` means inject nothing until the first real
  query. Summary is always scoped to a concrete namespace (`None` → `global`);
  it never lists across namespaces.

- **Hook injection reads the database directly, not through the daemon.**
  `hook context` opens the store itself (read-only operations only) via the
  same `Paths::db_file()` that `locusd` uses, so it honors `LOCUS_HOME` and is
  consistent with the daemon's database file. This keeps the path fast (no
  daemon spawn, no IPC round-trip) and read-only (no writer involvement); it is
  still the same `context_brief` generator the daemon uses for MCP.

- **`locus init` now also patches project docs.** A separate, marker-delimited
  doc protocol block (`DOC_PROTOCOL_START_MARKER`) is appended to
  `README.md`, `CONTRIBUTING.md`, and `AGENTS.md` **only when present** (never
  created). It is the passive fallback tier for agents without a hook system
  and always includes both the CLI form (`locus context "<task>"`) and the MCP
  tool form (`memory_search`).

### D-12 — U-011 secret redaction approach
Decisions made while implementing Security and Secret Redaction. Extends D-4
with the concrete shape of redact-or-warn.

- **Ship a curated gitleaks subset, not the full rule set.** The full gitleaks
  config is tuned for whole-repo scanning; its entropy-gated generic rules
  (e.g. `generic-api-key`) and file-path allowlists do not translate to short
  title/content fields. We vendor the distinctive-prefix rules that are
  unambiguous in prose and satisfy U-011's test matrix, compiled once via
  `include_str!` into a `LazyLock` set — nothing is fetched at runtime.

- **Entropy gates are implemented, not dropped.** Rules with a gitleaks
  `entropy` threshold only report matches whose Shannon entropy (bits/char,
  base-2, the same measure gitleaks uses) meets it. This is what stops
  repetitive/benign strings from tripping otherwise-plausible patterns.

- **Two regexes are ASCII-normalized (semantically equivalent).** Rust's
  `regex` crate treats `\w` as Unicode-aware, which blew the 10 MB compiled
  size limit on `[\w-]{50,1000}` and `\w{82}`. They are rewritten as
  `[0-9A-Za-z_-]` / `[0-9A-Za-z_]`, identical to Go's `\w` semantics, and the
  deviation is recorded in the vendored README.

- **Regex allowlists are kept; file-path allowlists are dropped.** Example-key
  allowlists (GCP docs keys) are compiled and honored because memory content
  legitimately quotes docs. `paths`-based allowlists are irrelevant to content
  scanning and omitted.

- **`password-in-url` is a curated addition.** gitleaks ships no dedicated
  password-in-URL rule, so one was added modeled on detect-secrets'
  `URL_CREDENTIALS` (any scheme, `scheme://user:pass@`), with attribution
  recorded. This is the only non-gitleaks rule.

- **Redaction is one choke point: `Store::insert_memory_checked`.**
  `security::redact_title_and_content` runs before validation-adjacent insert;
  raw `insert_memory` remains for internal/tooling use. The daemon writer
  thread and CLI `remember` both use the checked path, so every user-facing
  write is redact-or-warn by construction. Nothing is silently dropped and
  nothing is hard-rejected — a detected secret is replaced with a
  `[REDACTED:rule-id]` placeholder and a non-fatal warning.

- **Warnings carry rule ids and counts only — never the secret value.** The
  original secret never appears in warnings, IPC responses, or daemon logs;
  redaction happens before storage and before any logging. A test asserts the
  daemon log file contains no secret after a redacted write.

- **`--allow-secret` is explicit write-time consent.** CLI flag and MCP/IPC
  `allow_secret` field store the memory verbatim with no warning. There is no
  read-time or query-time opt-out; the secret is either redacted at write time
  or explicitly consented to.


