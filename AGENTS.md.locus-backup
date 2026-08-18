# AGENTS.md

You are working on Locus — a local-first, Rust-based long-term memory layer for AI coding agents.

## Read first

1. `docs/usecases/USECASES.md` — the backlog, dependency graph, status lifecycle, Definition of Done.
2. `docs/TECHSTACK.md` — locked technology choices.
3. `docs/DECISIONS.md` — rationale for past architectural calls. Check here before re-litigating anything (search engine, concurrency model, etc.) — if it's logged, it's settled.
4. `README.md` — product context and architecture.

## Repo structure

```text
/
  README.md  AGENTS.md  CONTRIBUTING.md  LICENSE
  docs/
    TECHSTACK.md
    DECISIONS.md
    usecases/USECASES.md
  crates/
    locus-core/   locus-cli/   locusd/   locus-mcp/   locus-testkit/
```

## Stack

- Rust stable, Cargo workspace. No TS/Node or Python in the core.
- SQLite (canonical store) with **FTS5** as the search engine — BM25 ranking, phrase/prefix search, no separate index process to keep in sync. This is the primary and current search engine, not a placeholder.
- Tantivy: not used. Rejected in favor of FTS5 to avoid a derived-index/drift problem and extra idle RSS. Do not reintroduce it without a new `DECISIONS.md` entry backed by benchmark data from U-012.
- IPC: Unix domain socket / named pipe via `interprocess`, blocking threads + a channel to the single writer — not tokio (see `DECISIONS.md`).
- MCP over stdio.

## Hard rules

- No network calls, no telemetry, no REST API unless explicitly approved.
- No secrets in logs, debug output, or stored memory content.
- Single writer path to SQLite.
- Respect the performance budget in `TECHSTACK.md` (CLI cold start < 50ms, warm search p95 < 20ms @ 100k memories, daemon idle RSS < 25MB). If a change breaks it, redesign — don't loosen the budget.
- Namespace isolation must never leak; changes touching search/storage need a test proving it.
- Don't swap search engine, storage, or IPC transport without a new `DECISIONS.md` entry.
- Hook adapters (U-015) are host-specific — don't build a generic cross-host hooks API. Each adapter maps one host's lifecycle events to a single internal "inject context" call.
- Hook-injected context and MCP-injected context must go through the exact same `ContextBrief` generator. Never build a second formatting/compression path.

## Workflow

1. Pick the lowest-numbered unblocked use case in `USECASES.md`.
2. Confirm all its dependencies are `Done` — don't build against an incomplete dependency.
3. Create a dedicated branch for that use case only, named `u-XXX-short-description` (for example, `u-006-daemon-ipc`).
4. Implement only that use case's scope. Don't expand it.
5. Write the tests listed in the use case (add more if useful, don't substitute).
6. Run:

```bash
cargo fmt --all
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all
```

7. Update the use case's scope checkmarks and docs affected by the change.
8. Set status to `Ready for Review`.
9. Stop. Wait for human approval.

## You may not, ever

- Set a use case's status to `Approved` or `Done` — human owner only, even if asked to "just mark it done."
- Skip tests, bypass unmet dependencies, or mark scope complete when it isn't.
- Scrape IDE logs, store secrets, or add a cloud/telemetry dependency.
- Use vector search as the primary search engine.
- Delete or silently overwrite `~/.locus/locus.db` or `~/.locus/index/` as part of a fix — recovery must be explicit, confirmed, and logged.

## If scope is ambiguous

Make the smallest reasonable assumption inside the use case's stated Problem/Solution, state it in the PR, and log it in `DECISIONS.md` if it's architecturally meaningful. Ambiguity is not license to expand scope.