# TECHSTACK.md — Locus Final Product Stack

This is the final product stack.

No MVP shortcuts.
No TypeScript core.
No Python core.
No Electron.
No cloud dependency.

---

## Primary Stack

| Layer | Choice | Reason |
|---|---|---|
| Language | Rust | Low memory, low CPU, fast startup, single binary |
| Package manager | Cargo | Standard Rust toolchain |
| CLI | clap | Mature Rust CLI framework |
| Canonical store | SQLite via rusqlite | Durable, embedded, battle-tested |
| Search engine | SQLite FTS5 | In-DB BM25 lexical index, no separate index process or drift |
| IPC | Unix domain socket | Fast local communication, no REST |
| IPC format | Newline-delimited JSON | Simple, debuggable, sufficient for small local messages |
| MCP transport | stdio | Standard for MCP clients |
| MCP protocol | JSON-RPC 2.0 over stdio | Standard MCP protocol |
| Serialization | serde + serde_json | Rust ecosystem standard |
| IDs | uuid | Stable unique identifiers |
| Time | time or chrono | Timestamp handling |
| Errors | thiserror + anyhow | Typed library errors, ergonomic app errors |
| Logging | tracing, disabled by default | Observability without overhead |
| Tests | cargo test | Standard |
| CLI tests | assert_cmd + predicates | CLI integration tests |
| Benchmarks | criterion | Performance benchmarks |
| Snapshot tests | insta | Stable output testing |
| Temp dirs | tempfile | Safe test isolation |

---

## Rejected Alternatives

### TypeScript / Node.js

Rejected for final product core.

Reasons:

- runtime overhead
- GC pauses
- heavier memory footprint
- less predictable latency
- requires Node installed
- less suitable for tiny local daemon
- less suitable for high-performance search index

TypeScript may be acceptable only for examples or docs, not core.

### Python

Rejected for final product core.

Reasons:

- runtime overhead
- packaging friction
- slower startup
- not ideal for low-resource local daemon

### Electron / Web Dashboard

Rejected.

Reasons:

- heavy RAM usage
- unnecessary for agent memory
- poor fit for local-first developer infrastructure

### REST API

Rejected as primary interface.

Reasons:

- unnecessary network surface
- more configuration
- less native for local tools
- CLI + Unix socket + MCP is cleaner

### Vector Database as Primary Store

Rejected as primary mechanism.

Reasons:

- poor exact keyword behavior for code identifiers
- unnecessary infrastructure
- embedding cost and latency
- weaker for metadata, timestamps, and exact technical terms

Vector embeddings may be optional later.

### Neo4j / Graph DB

Rejected for first final release.

Reasons:

- too heavy
- operational overhead
- not required for first version
- relationships can be modeled lightly in SQLite first

A graph layer may be added later if needed.

---

## Storage Architecture

### SQLite

SQLite is the canonical source of truth.

Recommended pragmas:

```sql
PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA temp_store = MEMORY;
PRAGMA mmap_size = 268435456;
PRAGMA cache_size = -20000;
PRAGMA busy_timeout = 5000;
```

Meaning:

- WAL allows concurrent reads and writes.
- `synchronous = NORMAL` gives good speed with acceptable durability.
- mmap improves read performance.
- cache size is modest to keep memory low.
- busy timeout avoids immediate lock errors.

### SQLite FTS5

SQLite FTS5 is the default and current search engine. It lives inside the same
`locus.db` file as the canonical store, so there is no separate index process,
no index directory, and no "written but not yet indexed" drift window.

Search sits behind a minimal `SearchEngine` trait (see `docs/DECISIONS.md`
D-1), so a different engine could be swapped in later without changing callers.

Indexed fields (FTS5 virtual table over the canonical rows):

```text
id
namespace
type
title
content
entities
importance
created_at
```

Index rules:

- SQLite is authoritative.
- The FTS5 table is kept transactionally consistent with the canonical rows.
- `locus reindex` must exist as a consistency-repair path (rebuild the FTS5
  table from canonical data); it is not needed in the common case.
- Bulk ingestion uses batched writes.
- Interactive writes should be searchable immediately or nearly immediately.
- Search uses read-only connections with prepared statements kept warm in the
  daemon.

### Tantivy (deferred upgrade path)

Tantivy is **not** used in the current design. It remains a benchmark-gated
upgrade path: it may be added later as a second `SearchEngine` implementation
only with evidence from U-012 and a new `docs/DECISIONS.md` entry. Adopting it
would introduce a separate index directory; FTS5 does not.

---

## Concurrency Model

### Daemon

`locusd` owns:

- SQLite write connection
- SQLite read connections
- FTS5 search execution over the open connection (warm prepared statements)
- search query execution
- context brief generation

### Writer Path

```text
request -> validate -> SQLite transaction (canonical rows + FTS5 table) -> ack
```

For bulk ingestion:

```text
queue -> batch SQLite inserts (canonical rows + FTS5 table) -> ack
```

### Reader Path

```text
search request -> FTS5 query -> fetch metadata if needed -> brief generation -> response
```

---

## Performance Budget

| Metric | Target |
|---|---:|
| CLI cold start | < 50 ms |
| Warm search p95 | < 20 ms for 100k memories |
| Save single memory p95 | < 15 ms |
| MCP tool response p95 | < 30 ms |
| Daemon idle RSS | < 25 MB |
| Daemon idle CPU | ~0% |
| Context brief generation | < 10 ms for top results |

If a design decision breaks these budgets, it must be changed.

---

## Suggested Cargo Workspace

```text
locus/
  Cargo.toml
  crates/
    locus-core/
    locus-cli/
    locusd/
    locus-mcp/
    locus-testkit/
```

### locus-core

Contains:

- memory schema
- SQLite store
- FTS5 search index
- brief generator
- search ranking
- conflict logic
- secret redaction

### locus-cli

Contains:

- `locus` binary
- human-facing commands

### locusd

Contains:

- daemon binary
- Unix socket server
- lifecycle management

### locus-mcp

Contains:

- `locus mcp` / `locus-mcp` binary
- MCP tool definitions (`memory_search`, `memory_save`, `memory_forget`,
  `memory_status`)
- blocking JSON-RPC 2.0 stdio transport (no Tokio — see DECISIONS.md D-9)
- daemon auto-start via the shared IPC client

### locus-testkit

Contains:

- fixtures
- dataset generator
- MCP test client
- benchmark helpers

---

## Security Defaults

- Database file permissions should be restrictive.
- Socket directory permissions should be restrictive.
- No network by default.
- No telemetry.
- No secret storage by default.
- Debug logs must not print secret content.
- Git ingestion must not store large diffs by default.

---

## Observability

Commands:

```bash
locus status
locus doctor
locus reindex
```

Future useful command:

```bash
locus search --explain
```

Debug logs must be disabled by default.

---

## Build Commands

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
cargo bench
cargo build --release
```

---

## Final Rule

If a technology choice increases memory usage, CPU usage, startup time, packaging complexity, or privacy risk without a strong benefit, reject it.
