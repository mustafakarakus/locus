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