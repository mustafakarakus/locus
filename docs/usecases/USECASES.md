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
| U-016 | Memory Visualization (Graph) |
| U-017 | Session Compaction Capture |

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
- [x] Human approval received.

---

## U-003: Search Engine (FTS5 default, trait-abstracted)

Status: Done 
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

- [x] Engines return relevance-ranked candidates only.
- [x] Recency and importance re-ranking lives in a shared layer above the engine.
- [x] Metadata filtering by namespace/type is applied in the query.
- [x] No `commit()` or `refresh()` in the trait; engines manage their own durability.
- [x] The trait must not expose FTS5-specific or Tantivy-specific options.

### FTS5 backend scope

- [x] Create FTS5 virtual table over memory title/content/entities.
- [x] Use native `bm25()` ranking.
- [x] Support phrase search.
- [x] Support prefix search.
- [x] Evaluate trigram tokenizer for partial/fuzzy-ish matching.
- [x] Keep FTS5 table transactionally consistent with the canonical store.
- [x] No separate index directory.
- [x] No separate rebuild machinery required in the common case.

### Re-ranking layer scope

- [x] Implement a shared re-ranker above the engine.
- [x] Apply recency boost.
- [x] Apply importance boost.
- [x] Produce final ordering returned to callers.
- [x] Re-ranker is engine-agnostic.

### Future Tantivy path

- [x] Tantivy is not implemented in this use case.
- [x] Tantivy may be added later as a second `SearchEngine` implementation.
- [x] Adding Tantivy requires benchmark evidence from U-012 first.

### Tests

- [x] Exact keyword search works.
- [x] Phrase search works.
- [x] Prefix search works.
- [x] Namespace filter prevents leakage.
- [x] Type filter works.
- [x] Identifier search works (function names, file names, API routes).
- [x] Partial-name search works.
- [x] Typo tolerance behavior is measured and documented.
- [x] Re-ranker orders newer/higher-importance results higher on close relevance.
- [x] FTS5 table stays consistent with canonical store after insert/update/delete.

### Definition of Done

- [x] All scope items complete.
- [x] All tests green.
- [x] Search benchmarks added.
- [x] Trait surface reviewed for minimalism.
- [x] Status changed to `Ready for Review`.
- [x] Human approval received.

---

## U-004: Context Brief Engine

Status: Done 
Priority: P0  
Depends On: U-003  
Blocks: U-005, U-007

### Problem

Agents need compressed context, not raw search results.

### Solution

Convert search results into a short Markdown brief.

### Scope

- [x] Implement `ContextBrief` generator.
- [x] Group memories by type.
- [x] Remove duplicate or near-duplicate entries.
- [x] Enforce token budget.
- [x] Return `NO_RELEVANT_MEMORY` when no useful result exists.
- [x] Keep output deterministic.
- [x] Include timestamps only when useful.
- [x] Do not output terminal colors.
- [x] Do not output raw JSON unless explicitly requested.

### Tests

- [x] Empty search returns `NO_RELEVANT_MEMORY`.
- [x] Decisions appear under `## Decisions`.
- [x] Preferences appear under `## Preferences`.
- [x] Output remains under target token budget.
- [x] Duplicate memories are merged.
- [x] Output is valid Markdown.
- [x] Deterministic output for same input.

### Definition of Done

- [x] All scope items complete.
- [x] All tests green.
- [x] Example brief documented.
- [x] Status changed to `Ready for Review`.
- [x] Human approval received.

---

## U-005: CLI Core Commands

Status: Done 
Priority: P0  
Depends On: U-003, U-004  
Blocks: U-006, U-008, U-009

### Problem

Humans need a fast terminal interface to Locus.

### Solution

Implement the `locus` CLI.

### Scope

- [x] Implement `locus remember`.
- [x] Implement `locus search`.
- [x] Implement `locus context`.
- [x] Implement `locus forget`.
- [x] Implement `locus status`.
- [x] Implement `locus doctor`.
- [x] Implement `locus reindex`.
- [x] Support `--namespace`.
- [x] Support `--type`.
- [x] Support `--importance`.
- [x] Support `--json`.
- [x] Support `--limit`.
- [x] Make CLI startup fast.
- [x] Make errors exit with non-zero status.
- [x] Make help text clear.

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

- [x] `remember` stores memory.
- [x] `search` finds memory.
- [x] `context` returns compressed Markdown.
- [x] `forget` deletes memory.
- [x] `status` shows database and search engine state.
- [x] Invalid command fails.
- [x] `--json` returns valid JSON.
- [x] CLI does not panic on bad input.

### Definition of Done

- [x] All scope items complete.
- [x] All tests green.
- [x] CLI help updated.
- [x] Status changed to `Ready for Review`.
- [x] Human approval received.

---

## U-006: Local Daemon and Cross-Platform IPC

Status: Done 
Priority: P0  
Depends On: U-003, U-004, U-005  
Blocks: U-007, U-009

> **Reviewer note (U-006 implementation):** Implemented `locusd` with the
> `interprocess` transport (Unix socket / Windows named pipe), the versioned
> newline-delimited JSON protocol, full lifecycle (foreground, idle shutdown,
> stop/SIGTERM/SIGINT, drain), auto-start, stale-socket recovery, and the
> `locus daemon {status,start,stop,restart}` subcommands. See DECISIONS.md
> **D-8** for the architecture choices (connect+ping health, CLI-spawn detached
> start, post-create socket `chmod`, `ctrlc` signals, `Condvar` idle wait).
> **Two knowingly-unchecked areas:** (1) Windows named-pipe items are coded via
> `cfg` but only exercised on Unix/macOS in CI here; (2) persistent
> connection + cached prepared statements are deferred (D-8) — the daemon warms
> the process/page-cache/WAL/mmap but still opens lightweight per-request SQLite
> handles. Rate limiting and a couple of dedicated latency benchmarks are also
> left for follow-up. Everything else below is implemented and tested.


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

- [x] Implement `locusd` as a Rust binary.
- [ ] Keep the SQLite connection open inside the daemon. *(deferred — D-8:
      warm process/page-cache/WAL/mmap today, persistent connection later)*
- [ ] Keep the search engine warm inside the daemon (via the `SearchEngine`
      trait — for the current FTS5 backend this means keeping prepared
      statements ready on the open connection, not a separate index reader).
      *(deferred — D-8)*
- [x] Keep context brief generator available inside the daemon.
- [x] Use a single-writer architecture.
- [x] Support concurrent read/search requests.
- [x] Prevent multiple daemon instances from running for the same user data directory.
- [x] Implement daemon lifecycle management:
  - start
  - stop
  - restart if stale
  - foreground mode
  - idle shutdown
- [x] Implement a local IPC transport abstraction.
- [x] Implement daemon health checking.
- [x] Ensure the daemon starts quickly.
- [x] Ensure the daemon uses minimal RAM when idle.
- [x] Ensure the daemon uses near-zero CPU when idle.
- [x] Ensure the daemon does not perform background network calls.
- [x] Ensure daemon logs do not contain secrets.

---

### IPC Transport Requirements

- [x] Define a transport trait in Rust.
- [x] Implement Unix domain socket transport.
- [x] Implement Windows named pipe transport.
- [x] Select the correct transport automatically by platform.
- [x] Do not use TCP by default.
- [x] Do not expose a REST API by default.
- [x] Do not bind to a public network interface.
- [ ] Allow a future optional loopback transport only behind explicit configuration.
- [x] Return a clear error if the platform transport is unavailable.

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

- [x] Use the runtime directory when available.
- [x] Fall back safely if runtime directory is unavailable.
- [x] Keep the socket path short.
- [x] Avoid paths with spaces where possible.

#### macOS

Preferred socket location:

```text
~/.locus/s.sock
```

or another short path if required.

Requirements:

- [x] Avoid long socket paths.
- [x] Respect macOS path length limits.
- [x] Keep the socket inside the user home directory by default.

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

- [x] Include protocol version.
- [x] Include request ID.
- [x] Include structured error codes.
- [x] Include a top-level `warnings` array in successful responses.
- [x] Warnings are non-fatal; `ok` remains `true` when warnings are present.
- [x] Errors are fatal; `ok` is `false` and `error` is present.
- [x] Cap warnings at 5 and dedupe by `code`.
- [x] MCP tool results propagate `warnings` so agents can surface them.
- [x] Reject unsupported protocol versions.
- [x] Reject oversized messages.
- [x] Close connection on repeated malformed input.
- [x] Do not panic on malformed JSON.
- [x] Do not expose raw database errors unless sanitized.
- [x] Do not log full request payloads by default.

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

- [x] `ping` returns daemon liveness and version.
- [x] `status` returns daemon, database, search engine, and transport state.
- [x] `remember` stores a memory.
- [x] `search` returns ranked search results.
- [x] `context` returns compressed Markdown brief.
- [x] `forget` deletes a memory.
- [x] `reindex` rebuilds the FTS5 search table from the canonical SQLite
      data. For the default FTS5 backend this is a consistency-repair
      operation on the same database file, not a rebuild of a separate index
      directory — it exists for the case where the FTS5 table and the
      canonical rows have drifted (e.g. after manual DB surgery or a bug),
      not as routine maintenance. If a second `SearchEngine` (e.g. Tantivy)
      is added later, `reindex` also rebuilds that engine's external index
      from SQLite.

---

### Daemon Lifecycle Requirements

- [x] Add `locusd --foreground`.
- [x] Add `locusd --no-idle-exit`.
- [x] Add `locusd --idle-timeout <seconds>`.
- [x] Add `locusd --log-level <level>`.
- [x] Support detached daemon startup.
- [x] Support clean shutdown on SIGTERM.
- [x] Support clean shutdown on SIGINT.
- [x] Support clean shutdown on client-requested stop.
- [x] Do not shut down while active requests are running.
- [x] Flush pending writes before shutdown.
- [x] Close database connections cleanly.
- [x] Close search engine resources cleanly (prepared statements for FTS5;
      index writer/readers for any future external-index engine).
- [x] Remove or invalidate IPC endpoint on clean shutdown where appropriate.

Idle behavior:

- [x] Default idle timeout should be configurable.
- [x] Suggested default idle timeout: 600 seconds.
- [x] Daemon exits after idle timeout if no requests arrive.
- [x] Daemon does not exit while search or write operations are active.
- [x] Idle shutdown must be logged.

---

### Auto-Start Requirements

The CLI and MCP server should not require the user to start the daemon manually.

Requirements:

- [x] Client first tries to connect to existing daemon.
- [x] If no daemon is running, client starts `locusd` automatically.
- [x] Auto-start must avoid starting multiple daemons.
- [x] Auto-start must work from CLI.
- [x] Auto-start must work from MCP server.
- [x] Auto-start must work after stale daemon state.
- [x] Auto-start must not require elevated permissions.
- [x] Auto-start must fail with a clear error if IPC endpoint is unavailable.

User-facing commands:

```bash
locus daemon status
locus daemon start
locus daemon stop
locus daemon restart
```

Requirements:

- [x] Add daemon management subcommands.
- [x] `locus daemon status` shows:
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
- [x] `locus daemon start` starts daemon if not running.
- [x] `locus daemon stop` stops daemon cleanly.
- [x] `locus daemon restart` restarts daemon cleanly.

---

### Locking and State Recovery Requirements

The daemon must recover safely from crashes.

Requirements:

- [x] Use a lock file or equivalent OS-specific lock. *(the bound IPC endpoint
      is the authority — bind + connect+ping, see D-8)*
- [x] Use a PID file or runtime metadata file.
- [x] Detect stale IPC endpoint.
- [ ] Detect stale lock file. *(N/A — endpoint is the lock; no separate lock file)*
- [ ] Detect stale PID file. *(PID file is advisory only, see D-8)*
- [x] Remove stale state only when safe.
- [x] Do not start a second daemon against the same data directory.
- [x] Recover if previous daemon crashed.
- [x] Validate database health on startup.
- [x] Validate FTS5 table consistency against canonical rows on startup.
- [x] Offer a `reindex` path if the FTS5 table is inconsistent or corrupt.
- [x] Do not silently delete user data.

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

- [x] Keep state inside a single user-owned directory.
- [x] Do not place logs outside the Locus directory by default.
- [x] Keep log files small or rotate them.
- [x] Do not write secrets into logs.

---

### Security Requirements

- [x] Restrict Locus data directory permissions.
- [x] Unix socket must be accessible only to the current user.
- [ ] Windows named pipe must be accessible only to the current user. *(coded, untested here)*
- [x] Database file must not be world-writable.
- [x] Log file must not be world-writable.
- [x] Daemon must not listen on public network interfaces.
- [x] Daemon must not require firewall exceptions.
- [x] Daemon must not send telemetry.
- [x] Daemon must not call external APIs.
- [ ] Daemon must reject unauthorized transport connections where OS support exists. *(relies on socket/dir 0600/0700 perms; no per-connection peer auth)*
- [x] IPC must not accept messages over an unbounded size.
- [x] IPC must enforce request timeout.
- [ ] IPC must enforce rate limiting for expensive operations where appropriate. *(follow-up)*

---

### Performance Requirements

Targets:

- [x] Daemon startup should be fast.
- [x] IPC connection should be fast.
- [ ] Warm search should not reopen the database or re-prepare statements. *(deferred — D-8)*
- [ ] Warm search p95 target remains under 20 ms for 100,000 memories. *(dedicated 100k benchmark is follow-up)*
- [ ] Single memory save p95 target remains under 15 ms. *(dedicated benchmark is follow-up)*
- [x] Idle daemon RSS target remains under 25 MB. *(measured ~9 MB idle)*
- [x] Idle daemon CPU usage should be effectively zero. *(measured 0.0%, Condvar wait)*
- [x] Daemon must not spin while waiting for requests.
- [x] Daemon must not repeatedly poll the filesystem when idle.

Requirements:

- [x] Use event-driven I/O.
- [x] Avoid unnecessary allocations in hot paths.
- [x] Avoid blocking the accept loop.
- [x] Use separate task/thread handling for requests.
- [x] Do not hold write locks during long read operations.
- [x] Do not block search on reindexing unless explicitly requested.

---

### VPS and Headless Requirements

Locus must work on headless Linux machines.

Requirements:

- [x] Daemon must run without a GUI.
- [x] Daemon must not require systemd by default.
- [x] Daemon must work when launched manually.
- [x] Daemon must work when auto-started by CLI or MCP.
- [x] Daemon must work over SSH sessions.
- [x] Daemon must not require network access.
- [x] Daemon must behave correctly when user logs out, depending on OS process model.
- [x] Document behavior for VPS usage.

Remote usage policy:

- [x] Remote memory serving is not included by default.
- [x] Daemon is local to the machine where it runs.
- [x] If AI tool and Locus are on different machines, that is out of scope for this use case.
- [x] Future remote transport must be explicitly approved, authenticated, and disabled by default.

---

### Observability Requirements

- [x] Add `ping` command.
- [x] Add `status` command.
- [x] Add daemon version to status.
- [x] Add protocol version to status.
- [x] Add transport type to status.
- [x] Add endpoint path to status.
- [x] Add uptime to status.
- [x] Add idle timeout to status.
- [x] Add database path to status.
- [x] Add search engine backend name to status (e.g. `fts5`).
- [x] Add last error, if any, to status.
- [x] Add debug logging disabled by default.
- [x] Ensure debug logs do not include secret content.

---

### Tests

#### Lifecycle Tests

- [x] Daemon starts in foreground mode.
- [x] Daemon starts detached.
- [x] Daemon shuts down cleanly on stop command.
- [x] Daemon shuts down cleanly on signal.
- [x] Daemon restarts cleanly.
- [x] Idle shutdown works.
- [ ] Daemon does not shut down during active request. *(drain logic implemented; no dedicated timing test)*
- [x] Daemon does not start twice for same data directory.

#### Transport Tests

- [x] Unix socket transport works on Linux.
- [x] Unix socket transport works on macOS.
- [ ] Named pipe transport works on Windows. *(coded via cfg, untested here)*
- [x] Transport selection is platform-correct.
- [x] Client can connect to daemon.
- [x] Client can reconnect after daemon restart.
- [x] Transport handles client disconnect.
- [x] Transport handles daemon crash.
- [x] Transport rejects oversized message.
- [x] Transport handles malformed JSON.

#### Stale State Tests

- [x] Stale socket file is detected.
- [ ] Stale lock file is detected. *(N/A — endpoint is the lock, D-8)*
- [ ] Stale PID file is detected. *(PID file advisory only, D-8)*
- [x] Daemon recovers after simulated crash.
- [x] Recovery does not delete database.
- [ ] Recovery does not rebuild the FTS5 table unless reindex is requested. *(startup only warns on drift; not asserted in a test)*

#### Concurrency Tests

- [x] Concurrent searches work.
- [x] Concurrent status requests work.
- [x] Concurrent writes are serialized.
- [x] Single writer prevents database corruption.
- [ ] Long search does not block daemon shutdown indefinitely. *(bounded drain timeout implemented; no dedicated test)*
- [ ] Long reindex does not block simple status requests indefinitely. *(status reads run inline, independent of the writer; no dedicated test)*

#### Protocol Tests

- [x] `ping` returns success.
- [x] Unknown command returns structured error.
- [x] Invalid payload returns structured error.
- [x] Unsupported protocol version returns structured error.
- [x] Request ID is preserved in response.
- [x] Success response shape is stable.
- [x] Error response shape is stable.

#### Command Tests

- [x] `remember` stores memory through daemon.
- [x] `search` returns results through daemon.
- [x] `context` returns compressed Markdown through daemon.
- [x] `forget` deletes memory through daemon.
- [x] `status` returns daemon state.
- [x] `reindex` rebuilds the FTS5 table safely from canonical data.
- [ ] `locus daemon status` reports correct state. *(manually verified; no automated CLI test yet)*
- [ ] `locus daemon start` starts daemon. *(manually verified)*
- [ ] `locus daemon stop` stops daemon. *(manually verified)*
- [ ] `locus daemon restart` restarts daemon. *(manually verified)*

#### Security Tests

- [x] Locus directory permissions are restrictive.
- [x] Database file permissions are restrictive.
- [x] Socket or pipe permissions are restrictive where supported.
- [x] Malformed IPC message does not crash daemon.
- [x] Oversized IPC message is rejected.
- [x] Daemon does not bind public network interface.
- [ ] Logs do not contain secret-like content. *(logs are metadata-only by construction; not asserted in a test)*

#### Performance Tests

- [x] Warm search latency benchmark exists. *(`locus-core/benches/search_bench.rs`)*
- [ ] Save latency benchmark exists. *(follow-up)*
- [ ] Context generation latency benchmark exists. *(follow-up)*
- [ ] Idle memory usage check exists. *(measured manually ~9 MB; no automated check)*
- [ ] Idle CPU usage check exists. *(measured manually 0%; no automated check)*
- [ ] Auto-start latency is measured. *(follow-up)*

---

### Behavior Documentation (as implemented)

**Cross-platform transport.** Selection is automatic per platform via the
`Endpoint` abstraction over the `interprocess` crate (D-3):

- Linux: Unix domain socket at `$XDG_RUNTIME_DIR/locus/locus.sock` when the
  runtime dir is available, else `~/.locus/s.sock`.
- macOS: Unix domain socket at `~/.locus/s.sock` (kept short to respect the
  `sun_path` length limit).
- Windows: per-user namespaced named pipe `locus-<user>-<hash>`.
- Custom/isolated instances (tests, `--data-dir`) use `<data-dir>/s.sock`.
- No TCP, no REST, no public bind. A future loopback transport would be
  opt-in and out of scope here.

**Lifecycle.** `locusd --foreground` runs in the foreground;
`--idle-timeout <secs>` (default 600) exits after inactivity;
`--no-idle-exit` disables that; `--log-level <level>` sets logging;
`--data-dir <dir>` isolates state. Shutdown is triggered by the `stop`
command, SIGINT, SIGTERM, or idle timeout, and always drains in-flight
requests (bounded by a drain timeout), flushes the single-writer queue, and
removes the PID and socket files. Auto-start: CLI/clients connect first and,
if nothing answers, spawn `locusd --foreground --data-dir <dir>` detached and
poll the endpoint until it answers (D-8).

**Stale-state recovery.** Liveness is decided by connect+ping, not PID probing
(D-8). On bind, an `AddrInUse` endpoint that answers a ping blocks a second
start; one that does not answer is treated as stale, its socket file removed,
and the bind retried. Recovery never deletes or rebuilds the database — a
drifted FTS5 table is only *warned* about on startup and repaired on explicit
`reindex`.

**Security defaults.** Data dir is created `0700`, the database and log files
`0600`, and the Unix socket `0600` (post-create `chmod`, D-8). No telemetry,
no external API calls, no public network interface, no firewall/admin needs.
IPC bounds each message to 1 MiB, enforces a request timeout, rejects
unsupported protocol versions, and closes a connection after repeated
malformed input. Logs are metadata-only and never include request payloads or
secrets.

**Resource usage.** Idle RSS measured ~9 MB (debug build) against the < 25 MB
budget; idle CPU 0.0% — the accept loop blocks and the idle monitor uses a
`Condvar` timed wait, so the daemon neither spins nor polls the filesystem.

**VPS / headless.** No GUI, systemd, or network required. The daemon runs when
launched manually or auto-started over an SSH session and is strictly local to
its machine. Remote/multi-machine serving is explicitly out of scope and would
require a separately approved, authenticated, default-off transport.

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

- [ ] All scope items complete. *(warm persistent connection deferred — D-8;
      Windows named pipe untested; see reviewer note)*
- [x] All tests green.
- [x] Cross-platform transport behavior documented.
- [x] Daemon lifecycle behavior documented.
- [x] Stale state recovery documented.
- [x] Security defaults documented.
- [x] Resource usage documented.
- [x] VPS/headless behavior documented.
- [x] No network exposure introduced by default.
- [x] Status changed to `Ready for Review`.
- [ ] Human approval received.

---

## U-007: MCP Server

Status: Done
Priority: P0  
Depends On: U-004, U-006  
Blocks: U-008, U-013

### Problem

AI tools need a standard way to use Locus.

### Solution

Implement MCP over stdio.

The MCP server should communicate with `locusd` through local IPC.

### Scope

- [x] Implement `locus mcp`.
- [x] Use stdio transport.
- [x] Implement MCP protocol framing according to official spec.
- [x] Implement tool `memory_search`.
- [x] Implement tool `memory_save`.
- [x] Implement tool `memory_forget`.
- [x] Implement tool `memory_status`.
- [x] Validate all inputs.
- [x] Return compressed Markdown by default from `memory_search`.
- [x] Return structured errors.
- [x] Do not expose raw database access.
- [x] Do not use network.
- [x] Ensure MCP server starts quickly.
- [x] Ensure MCP server works without a running daemon by auto-starting it.

### Tests

- [x] MCP server starts.
- [x] `memory_save` stores memory.
- [x] `memory_search` returns brief.
- [x] `memory_forget` deletes memory.
- [x] Invalid tool input returns structured error.
- [x] Unknown tool returns error.
- [x] Server shuts down cleanly.
- [x] Server works with auto-started daemon.

### Definition of Done

- [x] All scope items complete.
- [x] All tests green.
- [x] Example MCP config documented.
- [x] Status changed to `Ready for Review`.
- [x] Human approval received.

---

## U-008: Project Init and Agent Rules

Status: Done 
Priority: P0  
Depends On: U-005, U-007  
Blocks: U-013

### Problem

Agents will not use Locus automatically unless project rules tell them to.

### Solution

Implement `locus init`.

It should install a visible memory protocol into project rule files and MCP config.

### Scope

- [x] Implement `locus init`.
- [x] Detect current project type.
- [x] Detect `.cursorrules`.
- [x] Detect `CLAUDE.md`.
- [x] Detect `.clinerules`.
- [x] Detect MCP config location where possible.
- [x] Show diff before modifying files.
- [x] Require confirmation unless `--yes` is used.
- [x] Create backups before modifying files.
- [x] Append a visible `Locus Memory Protocol` block.
- [x] Make init idempotent.
- [x] Add MCP config for `locus mcp`.
- [x] Do not silently overwrite user content.

### Installed Protocol Must Say

- Before non-trivial code changes, call `memory_search`.
- Follow returned decisions and constraints.
- If a new decision is confirmed, call `memory_save`.
- Do not save secrets.
- If `NO_RELEVANT_MEMORY` is returned, continue normally.

### Tests

- [x] Fresh project gets rules block.
- [x] Existing file is appended safely.
- [x] Repeated init does not duplicate block.
- [x] Backup is created.
- [x] MCP config remains valid JSON.
- [x] `--yes` works non-interactively.
- [x] Init does not corrupt existing file.

### Definition of Done

- [x] All scope items complete.
- [x] All tests green.
- [x] Init flow documented.
- [x] Status changed to `Ready for Review`.
- [x] Human approval received.

---

## U-009: Git-Based Ingestion

Status: Done 
Priority: P1  
Depends On: U-005, U-006  
Blocks: U-013

### Problem

Locus should learn from real project changes without scraping IDE logs.

### Solution

Use standard Git hooks.

### Scope

- [x] Implement `locus hook install`.
- [x] Implement `locus hook uninstall`.
- [x] Install a `post-commit` hook.
- [x] Send commit metadata to daemon.
- [x] Store commit message.
- [x] Store changed file list.
- [x] Do not store full diffs by default.
- [x] Do not break existing Git hooks.
- [x] Make hook execution fast.
- [x] Ensure hook does not block commit for long.

### Tests

- [x] Hook install works.
- [x] Hook uninstall works.
- [x] Existing hook is preserved or safely wrapped.
- [x] Commit metadata creates memory.
- [x] Large diff is not stored.
- [x] Hook does not require network.
- [x] Hook failure does not corrupt Git commit.

### Definition of Done

- [x] All scope items complete.
- [x] All tests green.
- [x] Hook safety documented.
- [x] Status changed to `Ready for Review`.
- [x] Human approval received.

---

## U-010: Conflict, Decay, and Importance

Status: Done
Priority: P1  
Depends On: U-003  
Blocks: U-013

### Problem

Memories can become outdated or contradictory.

### Solution

Add time-aware ranking, importance ranking, and basic conflict detection.

### Scope

- [x] Add recency decay to ranking.
- [x] Add importance boost to ranking.
- [x] Detect likely conflicting memories.
- [x] Mark conflicts for review.
- [x] Add `locus conflicts` command.
- [x] Prefer newer confirmed decisions over older ones.
- [x] Do not silently delete memories.
- [x] Store conflict metadata.

### Tests

- [x] Newer memory ranks above older memory when relevance is close.
- [x] Higher importance ranks above lower importance when relevance is close.
- [x] Conflicting decisions are detected.
- [x] Conflict list can be shown.
- [x] Conflict detection does not delete data.

### Definition of Done

- [x] All scope items complete.
- [x] All tests green.
- [x] Ranking behavior documented.
- [x] Status changed to `Ready for Review`.
- [x] Human approval received.

---

## U-011: Security and Secret Redaction

Status: Done  
Priority: P0  
Depends On: U-002, U-005  
Blocks: U-013

### Problem

Memory systems can accidentally store secrets.

### Solution

Add local redaction and safe defaults using vendored, battle-tested pattern
sets rather than hand-rolled regexes.

### Scope

- [x] Vendor public rule sets from gitleaks and/or detect-secrets as static data.
- [x] Verify license of vendored pattern files and record attribution.
- [x] Detect common secret patterns using the vendored rules.
- [x] Default behavior is redact-or-warn, never silent drop.
- [x] Never hard-reject without flagging; surface a warning instead.
- [x] Add `--allow-secret` override for explicit user consent.
- [x] Emit warnings through the IPC/MCP `warnings` array.
- [x] Do not log secrets.
- [x] Do not include secrets in debug output.
- [x] Ensure database and socket permissions are restrictive.
- [x] Ensure namespace isolation.

### Tests

- [x] Real API key pattern (AWS `AKIA...`) is flagged.
- [x] Real token pattern (GitHub `ghp_...`) is flagged.
- [x] Private key block is flagged.
- [x] Password-in-URL is flagged.
- [x] UUID is NOT flagged.
- [x] Git commit SHA is NOT flagged.
- [x] Dependency-lock hash is NOT flagged.
- [x] Long benign base64-like string is NOT flagged.
- [x] Override flag `--allow-secret` works.
- [x] Warnings are returned in the response `warnings` array.
- [x] Debug logs do not contain secrets.
- [x] File permissions are safe.

### Definition of Done

- [x] All scope items complete.
- [x] All tests green.
- [x] Security rules documented.
- [x] Status changed to `Ready for Review`.
- [x] Human approval received.

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

Status: Done  
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

- [x] Host-specific adapters, not one generic hooks API.
- [x] Shared `ContextBrief` generation path with MCP.
- [x] Explicit default-query strategy for session-start injection.
- [x] Read-only, fast path with a small token budget.

### Adapter layer scope

- [x] Define a `HookAdapter` trait.
- [x] Implement `ClaudeCodeAdapter` first.
- [x] Each adapter maps its host's lifecycle events and payload shapes to a
      single internal call: inject context for a trigger.
- [x] Do not imply a single universal hooks API across hosts.
- [x] Additional host adapters are separate, explicit work.

### Default-query strategy scope

Session-start hooks have no user query yet. Decide and implement one strategy:

- [x] Inject a namespace-scoped project summary plus top decisions, OR
- [ ] Inject nothing until the first real query.

Default choice: namespace-scoped summary + top decisions, under a small token
budget.

- [x] Strategy is configurable per project namespace.
- [x] Output stays under a small token budget (default 200 tokens).
- [x] Uses the shared `ContextBrief` engine.

### Shared brief path scope

- [x] Hook injection and MCP `memory_search`/context use the same generator.
- [x] Single formatting/compression code path.
- [x] No divergent brief formats between triggers.

### Scope

- [x] Add `locus hook context` command for generic pre-reasoning injection.
- [x] Integrate with Claude Code lifecycle hooks (session-start / pre-tool).
- [x] Return compressed Markdown brief.
- [x] Return `NO_RELEVANT_MEMORY` when nothing applies.
- [x] Read-only; hooks never write memory.
- [x] Fast path; no index rebuild or heavy work on injection.
- [x] Extend `locus init` to write a delimited `Locus Memory Protocol` block
      into `README.md`, `CONTRIBUTING.md`, and `AGENTS.md` when present. The
      block instructs agents to run `locus context "<task>"` (CLI) or call
      `memory_search` (MCP) before making changes. It must be idempotent,
      clearly delimited so it can be detected and not duplicated, and must
      include both the CLI form and the MCP tool form so any agent can use
      whichever path is available to it. This is the passive fallback tier for
      agents that have no lifecycle hook system.

### Tests

- [x] Claude Code adapter translates lifecycle events correctly.
- [x] Session-start injection returns a namespace-scoped brief.
- [x] Hook output matches MCP brief output for the same query.
- [x] Hook injection stays under token budget.
- [x] Hook injection is read-only.
- [x] Unrelated session returns `NO_RELEVANT_MEMORY`.
- [x] Adapter failure degrades gracefully without blocking the host.
- [x] Doc protocol block is written to `README.md` when present.
- [x] Doc protocol block is written to `CONTRIBUTING.md` when present.
- [x] Doc protocol block is written to `AGENTS.md` when present.
- [x] Repeated `locus init` does not duplicate the doc protocol block.
- [x] Doc protocol block contains both the CLI form and the MCP tool form.

### Out of Scope

- Adapters for hosts without a hook system.
- Writing memory from hooks.
- Cross-host generic hook standard.

### Definition of Done

- [x] All scope items complete.
- [x] All tests green.
- [x] Adapter approach documented.
- [x] Default-query strategy documented.
- [x] Shared brief path verified.
- [x] Doc protocol block format documented.
- [x] Status changed to `Ready for Review`.
- [x] Human approval received.

---

## U-016: Memory Visualization (Graph)

Status: Done  
Priority: P2  
Depends On: U-002, U-003, U-006, U-011  
Blocks: None

### Problem

Memory is invisible. Users can store and retrieve decisions, constraints, bugs,
and context, but there is no way to *see* what Locus knows — how memories
relate, which ones are being used most, which are stale, or what is happening
live while an agent works.

### Solution

Add an on-demand local visualization: `locus graph` renders memories as an
interactive node graph where nodes are memories and edges are shared entities
or explicit links. Node size reflects retrieval frequency, node color reflects
recency.

Two modes:

- **Snapshot:** `locus graph` writes a fully self-contained HTML file (data
  embedded, all JS/CSS inlined — no CDN, works offline) and opens it. Regenerate
  to refresh.
- **Live:** `locus graph --live` spawns a separate `locus-viz` process that
  serves the page over loopback HTTP and pushes events (`memory_created`,
  `memory_searched`) over SSE so an already-open graph updates in real time.

The critical constraint: **reading data for the graph must never block or
deadlock the memory path** (search, save, context). Graph reads run on separate
read-only SQLite connections; live events are fire-and-forget on a bounded,
drop-on-backpressure channel; a slow or hung viz client must not stall
`locusd`.

### Access tracking scope

- [x] Add `access_count` and `last_accessed_at` to the memories schema (or a
      per-memory stats table).
- [x] Bump `access_count` on the retrieval path whenever a memory is surfaced
      to a caller (search/context).
- [x] The bump must be cheap and non-blocking (fire-and-forget/batched), never
      in the critical latency path of the search response.
- [x] Expose access stats through a read API so the graph can render
      "most visited" and "recently used" without a full scan.
- [x] Access stats must respect namespace isolation.

### Graph data scope

- [x] Build node set from memories.
- [x] Build edge set from shared entities and explicit links (no graph
      database; relationships come from SQLite joins, per TECHSTACK).
- [x] Support namespace scoping for the graph.
- [x] Edges never connect memories from different namespaces (cross-namespace
      entity sharing must not imply a relationship).
- [x] Renderer colors nodes by namespace (distinct hue per namespace, gray for
      `global`) with a legend naming each namespace.
- [x] Support `--expand <id>` to focus one memory and its immediate context.
- [x] Cap graph payloads (max nodes, max depth) so queries stay bounded.
- [x] Graph queries run on their own read-only SQLite connections, never on the
      shared warm search connection and never through the single-writer queue.
- [x] Graph queries must not acquire long-lived locks; WAL snapshot isolation
      means readers never block the single writer.

### Live event stream scope

- [x] Daemon emits events on retrieval and save (`memory_created`,
      `memory_searched`, `memory_used`).
- [x] Events go to a bounded broadcast channel; if a subscriber is slow or
      absent, events are dropped, never queued without bound.
- [x] Event emission must not block the daemon's request handling.
- [x] `locus-viz` subscribes to the daemon event stream over the existing IPC
      transport.

### `locus graph` CLI scope

- [x] Implement `locus graph` (snapshot mode).
- [x] Implement `locus graph --live` (spawns `locus-viz`, opens browser).
- [x] Implement `locus graph --namespace <ns>`.
- [x] Implement `locus graph --expand <id>`.
- [x] Write the self-contained HTML page.
- [x] Inline or vendor all JS/CSS; no CDN references; page must work offline.
- [x] Do not serve the page over HTTP in snapshot mode.
- [x] Do not introduce network calls in snapshot mode.

### `locus-viz` scope

- [x] Add `locus-viz` binary (separate crate or binary in the workspace).
- [x] Subscribes to daemon events over existing IPC.
- [x] Serves the HTML page over loopback HTTP (127.0.0.1 only) with SSE push.
- [x] Binds only on demand, while a viz client is connected.
- [x] Exits when the tab closes / no clients remain.
- [x] Must never run or linger when `locus graph` is not in live mode.
- [x] Rendering page live updates: new node fades in, visit counter ticks,
      usage pulses.
- [x] No telemetry, no analytics, no cloud calls from the page or server.

### Security scope

- [x] Loopback-only listener; never binds a public interface.
- [x] Page output must not contain secrets (relies on U-011 redaction, which
      happens at write time).
- [x] Live mode only exists while explicitly requested; nothing runs by
      default.
- [x] No REST API for memory; the viz HTTP listener serves the page and events
      only, never write operations.

### Non-blocking guarantees

- [x] Concurrent search/save continues with p95 within budget while a graph
      read is running (test).
- [x] A hung `locus-viz` client does not stall daemon requests (test).
- [x] Graph reads never deadlock against a concurrent write (test).
- [x] Event stream with no subscriber does not affect daemon performance
      (test).

### Tests

- [x] `access_count` increments when a memory is retrieved.
- [x] Access bump does not measurably delay the search response.
- [x] Snapshot mode writes a valid, self-contained HTML file with embedded data.
- [x] Snapshot mode makes no network calls.
- [x] Graph node set matches the memories in scope (namespace filter).
- [x] Graph edge set reflects shared entities.
- [x] Graph edges never cross namespaces (test).
- [x] `--expand <id>` shows the memory and its immediate links.
- [x] Live mode receives `memory_created` and renders a new node.
- [x] Live mode receives `memory_searched` and updates the visited counter.
- [x] Live SSE stream degrades gracefully when the viz client disconnects.
- [x] Daemon keeps serving search/save with p95 within budget during a
      long-running graph query.
- [x] A hung viz client does not block daemon shutdown or requests.
- [x] Graph query does not deadlock a concurrent save.
- [x] Viz listener binds only to loopback.
- [x] Page loads offline (no external requests).
- [x] Secrets are not present in the rendered graph.

### Out of Scope

- Hosted/shared/cloud dashboard.
- Editing memory from the graph.
- A graph database backend (Neo4j etc.); SQLite relationships remain the source.
- Real-time multi-user collaboration.
- Embedding the visualization inside `locusd` (stays a separate process).

### Definition of Done

- [x] All scope items complete.
- [x] All tests green.
- [x] Non-blocking guarantees verified.
- [x] Live event protocol documented.
- [x] Graph data model documented.
- [x] Status changed to `Ready for Review`.
- [x] Human approval received.

---

## U-017: Session Compaction Capture

Status: Backlog  
Priority: P1  
Depends On: U-004, U-006, U-007, U-011, U-015  
Blocks: None

### Problem

When an agent session's context window fills (~90-100%), the host compacts the
conversation into a summary and resets the working context. That summary is the
highest-signal record of what the session produced — decisions made, preferences
stated, constraints discovered — yet it is currently thrown away. The result is
lost long-term memory across tools (Cursor, Claude Code, Copilot, DeepSeek) and
across sessions: the same project gets re-litigated because nothing durable was
captured.

The compacted text must become durable, shared, retrievable memory — not raw
chat history, but discrete typed memories any agent can retrieve later.

### Solution

Add a host-independent capture path: when a host compacts its context, a
host-specific adapter forwards the compacted text to `locus-core`, where a
**deterministic rule-based extractor** (no LLM call) splits it into discrete,
typed memories and writes them through the existing store path.

This is the write-side mirror of U-015 (hook-based injection). U-015 injects a
brief before reasoning; U-017 captures when context is reset. Both map a host
lifecycle event to a single internal call into `locus-core`.

### Key decisions

- [ ] Extraction is rule-based and deterministic — no LLM dependency in the
      default capture path. Deterministic means the same compacted text always
      yields the same memories, regardless of which host or model produced it
      (a shared store must not vary per model).
- [ ] Extraction is zero-cost in the capture path (microseconds, local).
- [ ] Capture writes discrete typed memories through the existing store write
      path (namespace, redaction, dedupe) — never a raw summary blob.
- [ ] Optional LLM refinement may be added later behind a flag, never as a
      default dependency.

### Extractor scope

- [ ] Split the compacted summary into sentences.
- [ ] Classify each sentence into a `MemoryType` via cue-word patterns:
      decisions ("use X", "choose X", "standardize on X"), preferences
      ("prefer X", "avoid X"), constraints ("must not", "requires"), tasks
      ("in progress", "next step"), fallback to `Fact`/`Note`.
- [ ] Title = leading phrase of the sentence; content = the sentence.
- [ ] Importance by cue word strength (Decision/Preference higher, Note/Task
      lower).
- [ ] Extract entities via the existing `normalize_entities` plus simple
      proper-noun/camelCase tokens.
- [ ] Dedupe candidates against the shared store using the existing
      `normalize_for_dedupe` + `near_duplicate` logic so repeat captures do not
      multiply memories.
- [ ] Must handle host variance: the extractor normalizes summaries regardless
      of which host (Cursor/Claude Code/Copilot/DeepSeek) produced them. The
      extractor lives in `locus-core`; adapters only forward the text.

### Capture path scope

- [ ] Add a `capture` internal call in `locus-core` that takes compacted text
      plus a namespace and writes extracted memories.
- [ ] Add host-specific adapters (mirroring the U-015 adapter pattern) that map
      each host's compaction lifecycle event to the single `capture` call.
- [ ] Extraction runs on the compacted summary only; raw chat transcripts are
      never captured.
- [ ] Capture writes go through the same namespace scoping and U-011 redaction
      as any other write.
- [ ] Capture is bounded in cost: extract only durable categories
      (Decision/Preference/Constraint), skip session-transient task state.
- [ ] Capture must not block the host's compaction; it is fire-and-forget.

### Security scope

- [ ] Extracted memories pass through U-011 write-time redaction.
- [ ] Namespace isolation: captures are scoped to the session's namespace and
      never leak across namespaces.
- [ ] No network calls; capture is fully local.

### Tests

- [ ] Compaction text produces the expected typed memories (decisions,
      preferences, constraints).
- [ ] Same input always produces identical output (deterministic).
- [ ] Repeat capture of the same session does not duplicate memories.
- [ ] Host-specific adapter forwards its compaction payload correctly.
- [ ] Capture output matches the store write path (namespace + redaction
      applied).
- [ ] Unrelated/cue-word-free summary falls back to `Fact`/`Note`, never drops
      silently.
- [ ] Capture is read-only-safe: it never mutates beyond its own writes.
- [ ] Capture latency stays within the single-save p95 budget.
- [ ] Captured memories are retrievable by a second agent via the shared
      `ContextBrief` path.
- [ ] No secrets present in captured memories.

### Out of Scope

- LLM-based summarization or refinement in the default path (optional, behind
  a flag, later).
- Storing raw chat transcripts or the full compaction summary as a memory.
- Capturing transient task state (in-progress status, ephemeral next steps).
- Adapters for hosts without a compaction lifecycle event.

### Definition of Done

- [ ] All scope items complete.
- [ ] All tests green.
- [ ] Extractor heuristics documented.
- [ ] Adapter approach documented (mirrors U-015).
- [ ] Status changed to `Ready for Review`.
- [ ] Human approval received.