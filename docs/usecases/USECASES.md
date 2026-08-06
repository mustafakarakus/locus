# USECASES.md — Locus Final Product Use Cases

This is the final product implementation.

Do not downgrade this to an MVP.
Do not choose TypeScript, Node, Python, Electron, or a cloud service for the core.
Locus is a local-first, Rust-based memory layer for AI coding agents.

---

## Product Rules

- Local-first and offline by default.
- No telemetry, no analytics, no cloud calls.
- No secrets stored by default.
- Must work across AI tools: Cursor, Claude Code, Cline, and other MCP clients.
- Must feel instant.
- Must use very little RAM and CPU.
- Must prefer exact lexical search for code, identifiers, decisions, and project terms.
- Must not rely on hacky IDE log scraping.
- Must expose both:
  - a human-friendly CLI
  - an agent-friendly MCP interface
- Every feature must have tests.
- Human approval is required before a use case becomes `Done`.

---

## Engineering Rules

### Language and Core

- Rust stable.
- Cargo workspace.
- No JavaScript/TypeScript runtime in the product core.
- No Python runtime in the product core.
- No network access by default.
- No heavyweight database server.
- No external vector DB by default.

### Performance Budget

Targets:

- CLI cold start: under 50 ms on modern hardware.
- Warm search p95: under 20 ms for 100,000 memories.
- Single memory save p95: under 15 ms.
- MCP tool response p95: under 30 ms for warm search.
- Daemon idle RSS: under 25 MB.
- Daemon idle CPU: effectively 0%.
- Search must not block on heavy embedding generation.
- Bulk ingestion must not freeze interactive search.

### Database Rules

- SQLite is the canonical durable store.
- SQLite FTS5 is the default and current lexical search engine, living inside
  the same database file as the canonical store (see U-003).
- Search is hidden behind a minimal `SearchEngine` trait. A second engine
  (e.g. Tantivy) may be added later as an alternative implementation, but
  only with benchmark evidence from U-012 and a `DECISIONS.md` entry — it is
  not part of the current design.
- SQLite must use WAL mode.
- Use memory-mapped I/O where safe.
- Use prepared statements.
- Use a single writer path.
- Use read-only connections for search where possible.
- Bulk ingestion must batch writes and index commits.
- Interactive writes should be searchable immediately or nearly immediately.

### Search Rules

- Primary search is BM25/lexical through SQLite FTS5.
- Vector embeddings are optional and not required.
- Search must support:
  - exact terms
  - phrase search
  - prefix search
  - trigram-based partial/fuzzy-ish matching (typo tolerance is measured and
    documented per U-003, not guaranteed to the degree a dedicated fuzzy
    engine would provide)
  - namespace filter
  - type filter
  - recency boost
  - importance boost
- Search must not leak memories across namespaces.

### Memory Rules

Memory types:

```text
fact
decision
preference
task
bug
architecture
code
note
```

Memory object fields:

```text
id
namespace
type
title
content
entities
importance
source
created_at
updated_at
```

Default namespace:

```text
global
```

Project namespace format:

```text
project:<project-name>
```

### Context Brief Rules

- Agent-facing output must be compressed Markdown.
- Target brief size: under 400 tokens.
- Do not return raw chat logs.
- Do not return terminal colors.
- If no relevant memory exists, return exactly:

```text
NO_RELEVANT_MEMORY
```

Brief format:

```markdown
# Locus Memory Brief

## Decisions
- Short decision.

## Preferences
- Short preference.

## Constraints
- Short constraint.

## Tasks
- Short task.
```

---

## Status Lifecycle

```text
Backlog -> In Progress -> Ready for Review -> Approved -> Done
```

Additional status:

```text
Blocked
```

Rules:

- Coding agent may set `In Progress`.
- Coding agent may set `Ready for Review`.
- Coding agent must not set `Approved` or `Done`.
- Only the human owner may set `Approved` and `Done`.
- If dependencies are not done, the use case is blocked.

---

## Definition of Done

A use case is ready for review only when:

- [ ] All scope checkmarks are complete.
- [ ] Tests exist.
- [ ] Tests pass.
- [ ] `cargo fmt` has been run.
- [ ] `cargo clippy` passes with no warnings.
- [ ] Benchmarks exist where performance matters.
- [ ] Documentation is updated.
- [ ] No network calls are introduced.
- [ ] No secrets are logged or stored.
- [ ] Status is `Ready for Review`.

---

## Use Case Index

Canonical status, priority, dependencies, and blocks live inside each use case.

| ID | Title |
|---|---|
| U-001 | Rust Workspace Scaffold |
| U-002 | Canonical Memory Store |
| U-003 | Search Engine (FTS5 default, trait-abstracted) |
| U-004 | Context Brief Engine |
| U-005 | CLI Core Commands |
| U-006 | Local Daemon and Cross-Platform IPC |
| U-007 | MCP Server |
| U-008 | Project Init and Agent Rules |
| U-009 | Git-Based Ingestion |
| U-010 | Conflict, Decay, and Importance |
| U-011 | Security and Secret Redaction |
| U-012 | Performance Benchmarks |
| U-013 | End-to-End Cross-Agent Verification |
| U-014 | Packaging and Release |
| U-015 | Hook-Based Context Injection |

---

## Source of Truth

Each use case is the source of truth for its own:

- Status
- Priority
- Depends On
- Blocks
- Definition of Done

The index table is only a navigation list.
Do not store dependency or status information in the index table.

---

## U-001: Rust Workspace Scaffold

Status: Done  
Priority: P0  
Depends On: None  
Blocks: All

### Problem

Locus needs a clean Rust workspace for the final product.

### Solution

Create a Cargo workspace with separate crates for core, CLI, daemon, MCP, and test utilities.

### Scope

- [x] Create Cargo workspace.
- [x] Create crate `locus-core`.
- [x] Create crate `locus-cli`.
- [x] Create crate `locusd`.
- [x] Create crate `locus-mcp`.
- [x] Create crate `locus-testkit`.
- [x] Add shared error handling.
- [x] Add logging that is disabled by default.
- [x] Add `cargo fmt` configuration.
- [x] Add `cargo clippy` configuration.
- [x] Add placeholder tests.

### Tests

- [x] Workspace builds.
- [x] Placeholder unit test passes.
- [x] Clippy passes.
- [x] Formatting check passes.

### Definition of Done

- [x] All scope items complete.
- [x] All tests green.
- [x] Status changed to `Ready for Review`.
- [x] Human approval received.

---

## U-002: Canonical Memory Store

Status: Done
Priority: P0  
Depends On: U-001  
Blocks: U-003, U-004, U-005, U-006

### Problem

Locus needs a durable local source of truth for memories.

### Solution

Use SQLite as the canonical store.

### Scope

- [x] Create local database at `~/.locus/locus.db`.
- [x] Enable WAL mode.
- [x] Set safe pragmas for speed and durability.
- [x] Create `memories` table.
- [x] Create `entities` table.
- [x] Create `memory_entities` join table.
- [x] Create `migrations` table.
- [x] Implement migration runner.
- [x] Implement insert memory API.
- [x] Implement update memory API.
- [x] Implement delete memory API.
- [x] Implement get memory by ID API.
- [x] Implement list memories API.
- [x] Support namespace filtering.
- [x] Support type filtering.
- [x] Validate all memory fields.
- [x] Ensure database file permissions are restrictive.

### Tests

- [x] Database initialization is idempotent.
- [x] Migration runner can apply migrations twice safely.
- [x] Insert memory works.
- [x] Invalid memory type is rejected.
- [x] Missing namespace is rejected or defaulted safely.
- [x] Delete memory works.
- [x] Namespace isolation works.
- [x] Database file is not created world-writable.

### Definition of Done

- [x] All scope items complete.
- [x] All tests green.
- [x] Status changed to `Ready for Review`.
- [ ] Human approval received.

---

## U-003: Search Engine (FTS5 default, trait-abstracted)

Status: Backlog  
Priority: P0  
Depends On: U-002  
Blocks: U-004, U-005, U-006

### Problem

Locus needs fast lexical search for exact terms, code identifiers, decisions,
and project names, without introducing a second storage system that can drift
from the source of truth.

### Solution

Use SQLite FTS5 as the default search engine, inside the same SQLite database
as the canonical store. Hide it behind a minimal `SearchEngine` trait so a
different engine (e.g., Tantivy) can be swapped in later only if benchmarks
prove it necessary.

Keep the trait minimal. Do not pre-abstract features that only one
implementation uses.

### Trait design

The trait models Locus's query needs, not engine features.

```rust
pub trait SearchEngine: Send + Sync {
    fn search(&self, query: &Query) -> Result<Vec<Hit>>;
    fn upsert(&self, memory: &Memory) -> Result<()>;
    fn remove(&self, id: MemoryId) -> Result<()>;
}

pub struct Query {
    pub text: String,
    pub namespace: Option<String>,
    pub memory_type: Option<MemoryType>,
    pub limit: usize,
}

pub struct Hit {
    pub id: MemoryId,
    pub relevance: f32,
    pub snippet: String,
}
```

Rules:

- [ ] Engines return relevance-ranked candidates only.
- [ ] Recency and importance re-ranking lives in a shared layer above the engine.
- [ ] Metadata filtering by namespace/type is applied in the query.
- [ ] No `commit()` or `refresh()` in the trait; engines manage their own durability.
- [ ] The trait must not expose FTS5-specific or Tantivy-specific options.

### FTS5 backend scope

- [ ] Create FTS5 virtual table over memory title/content/entities.
- [ ] Use native `bm25()` ranking.
- [ ] Support phrase search.
- [ ] Support prefix search.
- [ ] Evaluate trigram tokenizer for partial/fuzzy-ish matching.
- [ ] Keep FTS5 table transactionally consistent with the canonical store.
- [ ] No separate index directory.
- [ ] No separate rebuild machinery required in the common case.

### Re-ranking layer scope

- [ ] Implement a shared re-ranker above the engine.
- [ ] Apply recency boost.
- [ ] Apply importance boost.
- [ ] Produce final ordering returned to callers.
- [ ] Re-ranker is engine-agnostic.

### Future Tantivy path

- [ ] Tantivy is not implemented in this use case.
- [ ] Tantivy may be added later as a second `SearchEngine` implementation.
- [ ] Adding Tantivy requires benchmark evidence from U-012 first.

### Tests

- [ ] Exact keyword search works.
- [ ] Phrase search works.
- [ ] Prefix search works.
- [ ] Namespace filter prevents leakage.
- [ ] Type filter works.
- [ ] Identifier search works (function names, file names, API routes).
- [ ] Partial-name search works.
- [ ] Typo tolerance behavior is measured and documented.
- [ ] Re-ranker orders newer/higher-importance results higher on close relevance.
- [ ] FTS5 table stays consistent with canonical store after insert/update/delete.

### Definition of Done

- [ ] All scope items complete.
- [ ] All tests green.
- [ ] Search benchmarks added.
- [ ] Trait surface reviewed for minimalism.
- [ ] Status changed to `Ready for Review`.
- [ ] Human approval received.

---

## U-004: Context Brief Engine

Status: Backlog  
Priority: P0  
Depends On: U-003  
Blocks: U-005, U-007

### Problem

Agents need compressed context, not raw search results.

### Solution

Convert search results into a short Markdown brief.

### Scope

- [ ] Implement `ContextBrief` generator.
- [ ] Group memories by type.
- [ ] Remove duplicate or near-duplicate entries.
- [ ] Enforce token budget.
- [ ] Return `NO_RELEVANT_MEMORY` when no useful result exists.
- [ ] Keep output deterministic.
- [ ] Include timestamps only when useful.
- [ ] Do not output terminal colors.
- [ ] Do not output raw JSON unless explicitly requested.

### Tests

- [ ] Empty search returns `NO_RELEVANT_MEMORY`.
- [ ] Decisions appear under `## Decisions`.
- [ ] Preferences appear under `## Preferences`.
- [ ] Output remains under target token budget.
- [ ] Duplicate memories are merged.
- [ ] Output is valid Markdown.
- [ ] Deterministic output for same input.

### Definition of Done

- [ ] All scope items complete.
- [ ] All tests green.
- [ ] Example brief documented.
- [ ] Status changed to `Ready for Review`.
- [ ] Human approval received.

---

## U-005: CLI Core Commands

Status: Backlog  
Priority: P0  
Depends On: U-003, U-004  
Blocks: U-006, U-008, U-009

### Problem

Humans need a fast terminal interface to Locus.

### Solution

Implement the `locus` CLI.

### Scope

- [ ] Implement `locus remember`.
- [ ] Implement `locus search`.
- [ ] Implement `locus context`.
- [ ] Implement `locus forget`.
- [ ] Implement `locus status`.
- [ ] Implement `locus doctor`.
- [ ] Implement `locus reindex`.
- [ ] Support `--namespace`.
- [ ] Support `--type`.
- [ ] Support `--importance`.
- [ ] Support `--json`.
- [ ] Support `--limit`.
- [ ] Make CLI startup fast.
- [ ] Make errors exit with non-zero status.
- [ ] Make help text clear.

### CLI Behavior

```bash
locus remember "Use Postgres for auth service" --type decision --namespace project:auth
locus search "auth database"
locus context "auth database"
locus forget <memory-id>
locus status
locus doctor
locus reindex
```

### Tests

- [ ] `remember` stores memory.
- [ ] `search` finds memory.
- [ ] `context` returns compressed Markdown.
- [ ] `forget` deletes memory.
- [ ] `status` shows database and search engine state.
- [ ] Invalid command fails.
- [ ] `--json` returns valid JSON.
- [ ] CLI does not panic on bad input.

### Definition of Done

- [ ] All scope items complete.
- [ ] All tests green.
- [ ] CLI help updated.
- [ ] Status changed to `Ready for Review`.
- [ ] Human approval received.

---

## U-006: Local Daemon and Cross-Platform IPC

Status: Backlog  
Priority: P0  
Depends On: U-003, U-004, U-005  
Blocks: U-007, U-009

### Problem

Opening SQLite and the search engine from many short-lived processes can
cause startup latency, duplicated memory usage, repeated warm-up work, and
lock contention.

Locus must feel instant when used by:

- the human CLI
- the MCP server
- Git hooks
- future automation

The system also needs to work reliably on:

- Linux
- macOS
- Windows
- headless machines
- VPS environments where the AI tool runs on the same machine

### Solution

Create a small local daemon, `locusd`, that keeps storage and search state warm.

The daemon must be:

- tiny
- quiet
- safe
- local-only
- cross-platform
- fast to connect to
- easy to auto-start
- easy to stop
- resilient to crashes and stale state

The daemon must not expose a network service by default.

Communication must happen through a local IPC transport.

Default transport per platform:

```text
Linux: Unix domain socket
macOS: Unix domain socket
Windows: named pipe
```

The IPC layer must be abstracted so the transport can be changed without changing the daemon protocol.

---

### Core Scope

- [ ] Implement `locusd` as a Rust binary.
- [ ] Keep the SQLite connection open inside the daemon.
- [ ] Keep the search engine warm inside the daemon (via the `SearchEngine`
      trait — for the current FTS5 backend this means keeping prepared
      statements ready on the open connection, not a separate index reader).
- [ ] Keep context brief generator available inside the daemon.
- [ ] Use a single-writer architecture.
- [ ] Support concurrent read/search requests.
- [ ] Prevent multiple daemon instances from running for the same user data directory.
- [ ] Implement daemon lifecycle management:
  - start
  - stop
  - restart if stale
  - foreground mode
  - idle shutdown
- [ ] Implement a local IPC transport abstraction.
- [ ] Implement daemon health checking.
- [ ] Ensure the daemon starts quickly.
- [ ] Ensure the daemon uses minimal RAM when idle.
- [ ] Ensure the daemon uses near-zero CPU when idle.
- [ ] Ensure the daemon does not perform background network calls.
- [ ] Ensure daemon logs do not contain secrets.

---

### IPC Transport Requirements

- [ ] Define a transport trait in Rust.
- [ ] Implement Unix domain socket transport.
- [ ] Implement Windows named pipe transport.
- [ ] Select the correct transport automatically by platform.
- [ ] Do not use TCP by default.
- [ ] Do not expose a REST API by default.
- [ ] Do not bind to a public network interface.
- [ ] Allow a future optional loopback transport only behind explicit configuration.
- [ ] Return a clear error if the platform transport is unavailable.

---

### IPC Endpoint Requirements

#### Linux

Preferred socket location:

```text
$XDG_RUNTIME_DIR/locus/locus.sock
```

Fallback:

```text
~/.locus/locus.sock
```

Requirements:

- [ ] Use the runtime directory when available.
- [ ] Fall back safely if runtime directory is unavailable.
- [ ] Keep the socket path short.
- [ ] Avoid paths with spaces where possible.

#### macOS

Preferred socket location:

```text
~/.locus/s.sock
```

or another short path if required.

Requirements:

- [ ] Avoid long socket paths.
- [ ] Respect macOS path length limits.
- [ ] Keep the socket inside the user home directory by default.

#### Windows

Preferred endpoint:

```text
\\.\pipe\locus-<user>
```

or another stable per-user pipe name.

Requirements:

- [ ] Use a per-user pipe name.
- [ ] Restrict access to the current user.
- [ ] Do not require administrator privileges.
- [ ] Handle pipe already-in-use cases.
- [ ] Handle pipe client disconnects cleanly.

---

### IPC Protocol Requirements

Use a versioned, local-only request/response protocol.

Default wire format:

```text
newline-delimited JSON
```

Each request must include:

```json
{
  "v": 1,
  "id": "request-id",
  "cmd": "search",
  "payload": {}
}
```

Each successful response must include:

```json
{
  "v": 1,
  "id": "request-id",
  "ok": true,
  "payload": {},
  "warnings": []
}
```

Error response:

```json
{
  "v": 1,
  "id": "request-id",
  "ok": false,
  "error": {
    "code": "invalid_input",
    "message": "query is required"
  }
}
```

Warning object shape:

```json
{
  "code": "possible_secret_redacted",
  "message": "A value matching an API key pattern was redacted before storage.",
  "field": "content"
}
```

Requirements:

- [ ] Include protocol version.
- [ ] Include request ID.
- [ ] Include structured error codes.
- [ ] Include a top-level `warnings` array in successful responses.
- [ ] Warnings are non-fatal; `ok` remains `true` when warnings are present.
- [ ] Errors are fatal; `ok` is `false` and `error` is present.
- [ ] Cap warnings at 5 and dedupe by `code`.
- [ ] MCP tool results propagate `warnings` so agents can surface them.
- [ ] Reject unsupported protocol versions.
- [ ] Reject oversized messages.
- [ ] Close connection on repeated malformed input.
- [ ] Do not panic on malformed JSON.
- [ ] Do not expose raw database errors unless sanitized.
- [ ] Do not log full request payloads by default.

Supported IPC commands:

```text
ping
status
remember
search
context
forget
reindex
```

Requirements:

- [ ] `ping` returns daemon liveness and version.
- [ ] `status` returns daemon, database, search engine, and transport state.
- [ ] `remember` stores a memory.
- [ ] `search` returns ranked search results.
- [ ] `context` returns compressed Markdown brief.
- [ ] `forget` deletes a memory.
- [ ] `reindex` rebuilds the FTS5 search table from the canonical SQLite
      data. For the default FTS5 backend this is a consistency-repair
      operation on the same database file, not a rebuild of a separate index
      directory — it exists for the case where the FTS5 table and the
      canonical rows have drifted (e.g. after manual DB surgery or a bug),
      not as routine maintenance. If a second `SearchEngine` (e.g. Tantivy)
      is added later, `reindex` also rebuilds that engine's external index
      from SQLite.

---

### Daemon Lifecycle Requirements

- [ ] Add `locusd --foreground`.
- [ ] Add `locusd --no-idle-exit`.
- [ ] Add `locusd --idle-timeout <seconds>`.
- [ ] Add `locusd --log-level <level>`.
- [ ] Support detached daemon startup.
- [ ] Support clean shutdown on SIGTERM.
- [ ] Support clean shutdown on SIGINT.
- [ ] Support clean shutdown on client-requested stop.
- [ ] Do not shut down while active requests are running.
- [ ] Flush pending writes before shutdown.
- [ ] Close database connections cleanly.
- [ ] Close search engine resources cleanly (prepared statements for FTS5;
      index writer/readers for any future external-index engine).
- [ ] Remove or invalidate IPC endpoint on clean shutdown where appropriate.

Idle behavior:

- [ ] Default idle timeout should be configurable.
- [ ] Suggested default idle timeout: 600 seconds.
- [ ] Daemon exits after idle timeout if no requests arrive.
- [ ] Daemon does not exit while search or write operations are active.
- [ ] Idle shutdown must be logged.

---

### Auto-Start Requirements

The CLI and MCP server should not require the user to start the daemon manually.

Requirements:

- [ ] Client first tries to connect to existing daemon.
- [ ] If no daemon is running, client starts `locusd` automatically.
- [ ] Auto-start must avoid starting multiple daemons.
- [ ] Auto-start must work from CLI.
- [ ] Auto-start must work from MCP server.
- [ ] Auto-start must work after stale daemon state.
- [ ] Auto-start must not require elevated permissions.
- [ ] Auto-start must fail with a clear error if IPC endpoint is unavailable.

User-facing commands:

```bash
locus daemon status
locus daemon start
locus daemon stop
locus daemon restart
```

Requirements:

- [ ] Add daemon management subcommands.
- [ ] `locus daemon status` shows:
  - running state
  - PID
  - transport type
  - endpoint path
  - database path
  - search engine backend (`fts5` by default; not a separate path — FTS5
    lives inside the database file. Only populated with a distinct path if
    a second, external-index engine is configured.)
  - idle timeout
  - version
- [ ] `locus daemon start` starts daemon if not running.
- [ ] `locus daemon stop` stops daemon cleanly.
- [ ] `locus daemon restart` restarts daemon cleanly.

---

### Locking and State Recovery Requirements

The daemon must recover safely from crashes.

Requirements:

- [ ] Use a lock file or equivalent OS-specific lock.
- [ ] Use a PID file or runtime metadata file.
- [ ] Detect stale IPC endpoint.
- [ ] Detect stale lock file.
- [ ] Detect stale PID file.
- [ ] Remove stale state only when safe.
- [ ] Do not start a second daemon against the same data directory.
- [ ] Recover if previous daemon crashed.
- [ ] Validate database health on startup.
- [ ] Validate FTS5 table consistency against canonical rows on startup.
- [ ] Offer a `reindex` path if the FTS5 table is inconsistent or corrupt.
- [ ] Do not silently delete user data.

Suggested state files:

```text
~/.locus/locus.db
~/.locus/locus.lock
~/.locus/locus.pid
~/.locus/logs/locusd.log
```

Note: there is no `~/.locus/index/` directory in the default configuration.
FTS5 search data lives inside `locus.db` itself. A separate index directory
would only be introduced if a second, external-index `SearchEngine` (e.g.
Tantivy) is added later — see U-003's "Future Tantivy path."

Requirements:

- [ ] Keep state inside a single user-owned directory.
- [ ] Do not place logs outside the Locus directory by default.
- [ ] Keep log files small or rotate them.
- [ ] Do not write secrets into logs.

---

### Security Requirements

- [ ] Restrict Locus data directory permissions.
- [ ] Unix socket must be accessible only to the current user.
- [ ] Windows named pipe must be accessible only to the current user.
- [ ] Database file must not be world-writable.
- [ ] Log file must not be world-writable.
- [ ] Daemon must not listen on public network interfaces.
- [ ] Daemon must not require firewall exceptions.
- [ ] Daemon must not send telemetry.
- [ ] Daemon must not call external APIs.
- [ ] Daemon must reject unauthorized transport connections where OS support exists.
- [ ] IPC must not accept messages over an unbounded size.
- [ ] IPC must enforce request timeout.
- [ ] IPC must enforce rate limiting for expensive operations where appropriate.

---

### Performance Requirements

Targets:

- [ ] Daemon startup should be fast.
- [ ] IPC connection should be fast.
- [ ] Warm search should not reopen the database or re-prepare statements.
- [ ] Warm search p95 target remains under 20 ms for 100,000 memories.
- [ ] Single memory save p95 target remains under 15 ms.
- [ ] Idle daemon RSS target remains under 25 MB.
- [ ] Idle daemon CPU usage should be effectively zero.
- [ ] Daemon must not spin while waiting for requests.
- [ ] Daemon must not repeatedly poll the filesystem when idle.

Requirements:

- [ ] Use event-driven I/O.
- [ ] Avoid unnecessary allocations in hot paths.
- [ ] Avoid blocking the accept loop.
- [ ] Use separate task/thread handling for requests.
- [ ] Do not hold write locks during long read operations.
- [ ] Do not block search on reindexing unless explicitly requested.

---

### VPS and Headless Requirements

Locus must work on headless Linux machines.

Requirements:

- [ ] Daemon must run without a GUI.
- [ ] Daemon must not require systemd by default.
- [ ] Daemon must work when launched manually.
- [ ] Daemon must work when auto-started by CLI or MCP.
- [ ] Daemon must work over SSH sessions.
- [ ] Daemon must not require network access.
- [ ] Daemon must behave correctly when user logs out, depending on OS process model.
- [ ] Document behavior for VPS usage.

Remote usage policy:

- [ ] Remote memory serving is not included by default.
- [ ] Daemon is local to the machine where it runs.
- [ ] If AI tool and Locus are on different machines, that is out of scope for this use case.
- [ ] Future remote transport must be explicitly approved, authenticated, and disabled by default.

---

### Observability Requirements

- [ ] Add `ping` command.
- [ ] Add `status` command.
- [ ] Add daemon version to status.
- [ ] Add protocol version to status.
- [ ] Add transport type to status.
- [ ] Add endpoint path to status.
- [ ] Add uptime to status.
- [ ] Add idle timeout to status.
- [ ] Add database path to status.
- [ ] Add search engine backend name to status (e.g. `fts5`).
- [ ] Add last error, if any, to status.
- [ ] Add debug logging disabled by default.
- [ ] Ensure debug logs do not include secret content.

---

### Tests

#### Lifecycle Tests

- [ ] Daemon starts in foreground mode.
- [ ] Daemon starts detached.
- [ ] Daemon shuts down cleanly on stop command.
- [ ] Daemon shuts down cleanly on signal.
- [ ] Daemon restarts cleanly.
- [ ] Idle shutdown works.
- [ ] Daemon does not shut down during active request.
- [ ] Daemon does not start twice for same data directory.

#### Transport Tests

- [ ] Unix socket transport works on Linux.
- [ ] Unix socket transport works on macOS.
- [ ] Named pipe transport works on Windows.
- [ ] Transport selection is platform-correct.
- [ ] Client can connect to daemon.
- [ ] Client can reconnect after daemon restart.
- [ ] Transport handles client disconnect.
- [ ] Transport handles daemon crash.
- [ ] Transport rejects oversized message.
- [ ] Transport handles malformed JSON.

#### Stale State Tests

- [ ] Stale socket file is detected.
- [ ] Stale lock file is detected.
- [ ] Stale PID file is detected.
- [ ] Daemon recovers after simulated crash.
- [ ] Recovery does not delete database.
- [ ] Recovery does not rebuild the FTS5 table unless reindex is requested.

#### Concurrency Tests

- [ ] Concurrent searches work.
- [ ] Concurrent status requests work.
- [ ] Concurrent writes are serialized.
- [ ] Single writer prevents database corruption.
- [ ] Long search does not block daemon shutdown indefinitely.
- [ ] Long reindex does not block simple status requests indefinitely.

#### Protocol Tests

- [ ] `ping` returns success.
- [ ] Unknown command returns structured error.
- [ ] Invalid payload returns structured error.
- [ ] Unsupported protocol version returns structured error.
- [ ] Request ID is preserved in response.
- [ ] Success response shape is stable.
- [ ] Error response shape is stable.

#### Command Tests

- [ ] `remember` stores memory through daemon.
- [ ] `search` returns results through daemon.
- [ ] `context` returns compressed Markdown through daemon.
- [ ] `forget` deletes memory through daemon.
- [ ] `status` returns daemon state.
- [ ] `reindex` rebuilds the FTS5 table safely from canonical data.
- [ ] `locus daemon status` reports correct state.
- [ ] `locus daemon start` starts daemon.
- [ ] `locus daemon stop` stops daemon.
- [ ] `locus daemon restart` restarts daemon.

#### Security Tests

- [ ] Locus directory permissions are restrictive.
- [ ] Database file permissions are restrictive.
- [ ] Socket or pipe permissions are restrictive where supported.
- [ ] Malformed IPC message does not crash daemon.
- [ ] Oversized IPC message is rejected.
- [ ] Daemon does not bind public network interface.
- [ ] Logs do not contain secret-like content.

#### Performance Tests

- [ ] Warm search latency benchmark exists.
- [ ] Save latency benchmark exists.
- [ ] Context generation latency benchmark exists.
- [ ] Idle memory usage check exists.
- [ ] Idle CPU usage check exists.
- [ ] Auto-start latency is measured.

---

### Out of Scope

The following are not included in this use case:

- Remote daemon serving by default.
- Public HTTP API.
- REST API.
- gRPC API.
- TLS certificate management.
- Multi-user shared daemon.
- Cloud sync.
- Team memory sharing.
- Automatic IDE log scraping.
- Windows service installation by default.
- systemd unit installation by default.

These may be considered later only as explicit separate use cases.

---

### Definition of Done

- [ ] All scope items complete.
- [ ] All tests green.
- [ ] Cross-platform transport behavior documented.
- [ ] Daemon lifecycle behavior documented.
- [ ] Stale state recovery documented.
- [ ] Security defaults documented.
- [ ] Resource usage documented.
- [ ] VPS/headless behavior documented.
- [ ] No network exposure introduced by default.
- [ ] Status changed to `Ready for Review`.
- [ ] Human approval received.

---

## U-007: MCP Server

Status: Backlog  
Priority: P0  
Depends On: U-004, U-006  
Blocks: U-008, U-013

### Problem

AI tools need a standard way to use Locus.

### Solution

Implement MCP over stdio.

The MCP server should communicate with `locusd` through local IPC.

### Scope

- [ ] Implement `locus mcp`.
- [ ] Use stdio transport.
- [ ] Implement MCP protocol framing according to official spec.
- [ ] Implement tool `memory_search`.
- [ ] Implement tool `memory_save`.
- [ ] Implement tool `memory_forget`.
- [ ] Implement tool `memory_status`.
- [ ] Validate all inputs.
- [ ] Return compressed Markdown by default from `memory_search`.
- [ ] Return structured errors.
- [ ] Do not expose raw database access.
- [ ] Do not use network.
- [ ] Ensure MCP server starts quickly.
- [ ] Ensure MCP server works without a running daemon by auto-starting it.

### Tests

- [ ] MCP server starts.
- [ ] `memory_save` stores memory.
- [ ] `memory_search` returns brief.
- [ ] `memory_forget` deletes memory.
- [ ] Invalid tool input returns structured error.
- [ ] Unknown tool returns error.
- [ ] Server shuts down cleanly.
- [ ] Server works with auto-started daemon.

### Definition of Done

- [ ] All scope items complete.
- [ ] All tests green.
- [ ] Example MCP config documented.
- [ ] Status changed to `Ready for Review`.
- [ ] Human approval received.

---

## U-008: Project Init and Agent Rules

Status: Backlog  
Priority: P0  
Depends On: U-005, U-007  
Blocks: U-013

### Problem

Agents will not use Locus automatically unless project rules tell them to.

### Solution

Implement `locus init`.

It should install a visible memory protocol into project rule files and MCP config.

### Scope

- [ ] Implement `locus init`.
- [ ] Detect current project type.
- [ ] Detect `.cursorrules`.
- [ ] Detect `CLAUDE.md`.
- [ ] Detect `.clinerules`.
- [ ] Detect MCP config location where possible.
- [ ] Show diff before modifying files.
- [ ] Require confirmation unless `--yes` is used.
- [ ] Create backups before modifying files.
- [ ] Append a visible `Locus Memory Protocol` block.
- [ ] Make init idempotent.
- [ ] Add MCP config for `locus mcp`.
- [ ] Do not silently overwrite user content.

### Installed Protocol Must Say

- Before non-trivial code changes, call `memory_search`.
- Follow returned decisions and constraints.
- If a new decision is confirmed, call `memory_save`.
- Do not save secrets.
- If `NO_RELEVANT_MEMORY` is returned, continue normally.

### Tests

- [ ] Fresh project gets rules block.
- [ ] Existing file is appended safely.
- [ ] Repeated init does not duplicate block.
- [ ] Backup is created.
- [ ] MCP config remains valid JSON.
- [ ] `--yes` works non-interactively.
- [ ] Init does not corrupt existing file.

### Definition of Done

- [ ] All scope items complete.
- [ ] All tests green.
- [ ] Init flow documented.
- [ ] Status changed to `Ready for Review`.
- [ ] Human approval received.

---

## U-009: Git-Based Ingestion

Status: Backlog  
Priority: P1  
Depends On: U-005, U-006  
Blocks: U-013

### Problem

Locus should learn from real project changes without scraping IDE logs.

### Solution

Use standard Git hooks.

### Scope

- [ ] Implement `locus hook install`.
- [ ] Implement `locus hook uninstall`.
- [ ] Install a `post-commit` hook.
- [ ] Send commit metadata to daemon.
- [ ] Store commit message.
- [ ] Store changed file list.
- [ ] Do not store full diffs by default.
- [ ] Do not break existing Git hooks.
- [ ] Make hook execution fast.
- [ ] Ensure hook does not block commit for long.

### Tests

- [ ] Hook install works.
- [ ] Hook uninstall works.
- [ ] Existing hook is preserved or safely wrapped.
- [ ] Commit metadata creates memory.
- [ ] Large diff is not stored.
- [ ] Hook does not require network.
- [ ] Hook failure does not corrupt Git commit.

### Definition of Done

- [ ] All scope items complete.
- [ ] All tests green.
- [ ] Hook safety documented.
- [ ] Status changed to `Ready for Review`.
- [ ] Human approval received.

---

## U-010: Conflict, Decay, and Importance

Status: Backlog  
Priority: P1  
Depends On: U-003  
Blocks: U-013

### Problem

Memories can become outdated or contradictory.

### Solution

Add time-aware ranking, importance ranking, and basic conflict detection.

### Scope

- [ ] Add recency decay to ranking.
- [ ] Add importance boost to ranking.
- [ ] Detect likely conflicting memories.
- [ ] Mark conflicts for review.
- [ ] Add `locus conflicts` command.
- [ ] Prefer newer confirmed decisions over older ones.
- [ ] Do not silently delete memories.
- [ ] Store conflict metadata.

### Tests

- [ ] Newer memory ranks above older memory when relevance is close.
- [ ] Higher importance ranks above lower importance when relevance is close.
- [ ] Conflicting decisions are detected.
- [ ] Conflict list can be shown.
- [ ] Conflict detection does not delete data.

### Definition of Done

- [ ] All scope items complete.
- [ ] All tests green.
- [ ] Ranking behavior documented.
- [ ] Status changed to `Ready for Review`.
- [ ] Human approval received.

---

## U-011: Security and Secret Redaction

Status: Backlog  
Priority: P0  
Depends On: U-002, U-005  
Blocks: U-013

### Problem

Memory systems can accidentally store secrets.

### Solution

Add local redaction and safe defaults using vendored, battle-tested pattern
sets rather than hand-rolled regexes.

### Scope

- [ ] Vendor public rule sets from gitleaks and/or detect-secrets as static data.
- [ ] Verify license of vendored pattern files and record attribution.
- [ ] Detect common secret patterns using the vendored rules.
- [ ] Default behavior is redact-or-warn, never silent drop.
- [ ] Never hard-reject without flagging; surface a warning instead.
- [ ] Add `--allow-secret` override for explicit user consent.
- [ ] Emit warnings through the IPC/MCP `warnings` array.
- [ ] Do not log secrets.
- [ ] Do not include secrets in debug output.
- [ ] Ensure database and socket permissions are restrictive.
- [ ] Ensure namespace isolation.

### Tests

- [ ] Real API key pattern (AWS `AKIA...`) is flagged.
- [ ] Real token pattern (GitHub `ghp_...`) is flagged.
- [ ] Private key block is flagged.
- [ ] Password-in-URL is flagged.
- [ ] UUID is NOT flagged.
- [ ] Git commit SHA is NOT flagged.
- [ ] Dependency-lock hash is NOT flagged.
- [ ] Long benign base64-like string is NOT flagged.
- [ ] Override flag `--allow-secret` works.
- [ ] Warnings are returned in the response `warnings` array.
- [ ] Debug logs do not contain secrets.
- [ ] File permissions are safe.

### Definition of Done

- [ ] All scope items complete.
- [ ] All tests green.
- [ ] Security rules documented.
- [ ] Status changed to `Ready for Review`.
- [ ] Human approval received.

---

## U-012: Performance Benchmarks

Status: Backlog  
Priority: P0  
Depends On: U-003, U-006  
Blocks: U-014

### Problem

Locus must prove it is fast and lightweight.

### Solution

Add benchmarks and resource checks.

### Scope

- [ ] Add dataset generator.
- [ ] Generate 1,000 memories.
- [ ] Generate 10,000 memories.
- [ ] Generate 100,000 memories.
- [ ] Benchmark search latency.
- [ ] Benchmark save latency.
- [ ] Benchmark context generation.
- [ ] Benchmark daemon idle memory.
- [ ] Benchmark CLI startup.
- [ ] Add `locus bench` or Cargo bench targets.
- [ ] Record p50, p95, p99.
- [ ] Fail benchmark suite if budget is exceeded.
- [ ] Include the identifier/partial-name/typo-tolerance queries called out
      in U-003, specifically to gather the evidence needed before Tantivy
      would ever be considered.

### Tests

- [ ] Benchmark dataset generator works.
- [ ] Search benchmark runs.
- [ ] Save benchmark runs.
- [ ] Memory usage report is generated.
- [ ] Benchmark results are reproducible.

### Definition of Done

- [ ] All scope items complete.
- [ ] Benchmarks pass.
- [ ] Performance report documented.
- [ ] Status changed to `Ready for Review`.
- [ ] Human approval received.

---

## U-013: End-to-End Cross-Agent Verification

Status: Backlog  
Priority: P1  
Depends On: U-007, U-008, U-009, U-010, U-011, U-015  
Blocks: U-014

### Problem

We need proof that memory persists across tools and sessions.

### Solution

Create an E2E test that simulates two agents.

### Scenario

1. Agent A saves a decision.
2. Agent B starts later.
3. Agent B searches memory.
4. Agent B receives compressed brief.
5. Agent B respects the decision.
6. No raw README or chat history is required.

### Scope

- [ ] Create fixture project.
- [ ] Simulate Agent A using MCP.
- [ ] Simulate Agent B using MCP.
- [ ] Verify saved decision is retrieved.
- [ ] Verify brief is compressed.
- [ ] Verify namespace isolation.
- [ ] Verify secret redaction.
- [ ] Verify no network usage.
- [ ] Verify Git ingestion can add memory.
- [ ] Verify conflict handling does not break retrieval.

### Tests

- [ ] Saved decision is retrievable by second agent.
- [ ] Unrelated query returns `NO_RELEVANT_MEMORY`.
- [ ] Project namespace prevents leakage.
- [ ] Secret-like input is not stored raw.
- [ ] Output stays under token budget.
- [ ] MCP contract remains stable.

### Definition of Done

- [ ] All scope items complete.
- [ ] All tests green.
- [ ] Example transcript documented.
- [ ] Status changed to `Ready for Review`.
- [ ] Human approval received.

---

## U-014: Packaging and Release

Status: Backlog  
Priority: P1  
Depends On: U-012, U-013  
Blocks: None

### Problem

Users need a clean way to install Locus.

### Solution

Package Locus as a native binary.

### Scope

- [ ] Build release binaries.
- [ ] Add shell completions.
- [ ] Add install script.
- [ ] Add uninstall script.
- [ ] Add Homebrew formula.
- [ ] Add Cargo install support.
- [ ] Add `locus doctor`.
- [ ] Document upgrade path.
- [ ] Document database backup path.
- [ ] Ensure no runtime dependencies are required.

### Tests

- [ ] Install script works in temporary directory.
- [ ] Uninstall script works.
- [ ] Binary starts.
- [ ] Shell completions generate.
- [ ] `locus doctor` passes on clean install.

### Definition of Done

- [ ] All scope items complete.
- [ ] All tests green.
- [ ] Release process documented.
- [ ] Status changed to `Ready for Review`.
- [ ] Human approval received.

---

## U-015: Hook-Based Context Injection

Status: Backlog  
Priority: P0  
Depends On: U-004, U-006, U-007  
Blocks: U-013

### Problem

MCP is pull-based. `memory_search` and `memory_save` only fire when the model
decides to call them. Prompt-based instructions are best-effort and degrade on
smaller models and long sessions. For memory to be load-bearing, context must
be injected by a platform guarantee, not by model compliance.

### Solution

Provide host-specific lifecycle hook adapters that inject a `ContextBrief`
before the model starts reasoning, independent of whether the model calls a
tool.

This is a different trigger over the same backend. It must reuse the exact
same `ContextBrief` generation path as the MCP interface.

### Key decisions

- [ ] Host-specific adapters, not one generic hooks API.
- [ ] Shared `ContextBrief` generation path with MCP.
- [ ] Explicit default-query strategy for session-start injection.
- [ ] Read-only, fast path with a small token budget.

### Adapter layer scope

- [ ] Define a `HookAdapter` trait.
- [ ] Implement `ClaudeCodeAdapter` first.
- [ ] Each adapter maps its host's lifecycle events and payload shapes to a
      single internal call: inject context for a trigger.
- [ ] Do not imply a single universal hooks API across hosts.
- [ ] Additional host adapters are separate, explicit work.

### Default-query strategy scope

Session-start hooks have no user query yet. Decide and implement one strategy:

- [ ] Inject a namespace-scoped project summary plus top decisions, OR
- [ ] Inject nothing until the first real query.

Default choice: namespace-scoped summary + top decisions, under a small token
budget.

- [ ] Strategy is configurable per project namespace.
- [ ] Output stays under a small token budget (default 200 tokens).
- [ ] Uses the shared `ContextBrief` engine.

### Shared brief path scope

- [ ] Hook injection and MCP `memory_search`/context use the same generator.
- [ ] Single formatting/compression code path.
- [ ] No divergent brief formats between triggers.

### Scope

- [ ] Add `locus hook context` command for generic pre-reasoning injection.
- [ ] Integrate with Claude Code lifecycle hooks (session-start / pre-tool).
- [ ] Return compressed Markdown brief.
- [ ] Return `NO_RELEVANT_MEMORY` when nothing applies.
- [ ] Read-only; hooks never write memory.
- [ ] Fast path; no index rebuild or heavy work on injection.

### Tests

- [ ] Claude Code adapter translates lifecycle events correctly.
- [ ] Session-start injection returns a namespace-scoped brief.
- [ ] Hook output matches MCP brief output for the same query.
- [ ] Hook injection stays under token budget.
- [ ] Hook injection is read-only.
- [ ] Unrelated session returns `NO_RELEVANT_MEMORY`.
- [ ] Adapter failure degrades gracefully without blocking the host.

### Out of Scope

- Adapters for hosts without a hook system.
- Writing memory from hooks.
- Cross-host generic hook standard.

### Definition of Done

- [ ] All scope items complete.
- [ ] All tests green.
- [ ] Adapter approach documented.
- [ ] Default-query strategy documented.
- [ ] Shared brief path verified.
- [ ] Status changed to `Ready for Review`.
- [ ] Human approval received.