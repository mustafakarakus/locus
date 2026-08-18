# Contributing to Locus

Thanks for your interest in Locus. This project follows a stricter-than-usual
process because it's designed to be built collaboratively by both humans and
AI coding agents. Read this fully before opening a PR — the workflow is not
the typical "fork, code, PR" flow.

---

## Before you write any code

Every change to Locus must map to a **use case** in
[`docs/usecases/USECASES.md`](docs/usecases/USECASES.md).

- Don't open a PR without a corresponding use case ID (e.g. `U-006`).
- If your change doesn't fit an existing use case, open an issue proposing a
  new one first. Include: Problem, Solution, Scope, Tests — the same shape
  every existing use case follows.
- Check the current status and `Depends On` list inside the relevant use case
  section before starting — dependency info lives per use case, not in the
  index table at the top of `USECASES.md`.

This exists so that both human and AI contributors are working from the same
source of truth, and so scope doesn't drift mid-implementation.

---

## Required reading

| File | What it covers |
|---|---|
| [`README.md`](README.md) | Product context and architecture |
| [`AGENTS.md`](AGENTS.md) | Rules for AI coding agents working in this repo |
| [`docs/TECHSTACK.md`](docs/TECHSTACK.md) | Locked technology choices and why alternatives were rejected |
| [`docs/DECISIONS.md`](docs/DECISIONS.md) | Log of architectural decisions and their rationale |
| [`docs/usecases/USECASES.md`](docs/usecases/USECASES.md) | The full use case backlog, status, and Definition of Done |

If you're proposing something that contradicts `TECHSTACK.md` (a new
dependency, a different storage engine, a network call, etc.), open a
discussion issue first. Don't submit the code change and the architecture
change in the same PR.

---

## Status lifecycle

```text
Backlog -> In Progress -> Ready for Review -> Approved -> Done
```

Plus `Blocked`.

- You (human or agent) may move a use case to `In Progress` or
  `Ready for Review`.
- **Only a human project owner may set `Approved` or `Done`.** This applies
  equally to AI agents contributing code — no autonomous merge to `Done`.
- If a use case's dependencies aren't `Done`, it's `Blocked`. Don't build on
  top of an incomplete dependency and expect it to be merged.

---

## Definition of Done

A PR is not ready for review unless **all** of the following are true. This
is checked, not assumed:

- [ ] All scope checkboxes in the use case are complete.
- [ ] Tests exist for the use case's listed test scenarios.
- [ ] Tests pass locally and in CI.
- [ ] `cargo fmt --all` has been run.
- [ ] `cargo clippy --all-targets --all-features -- -D warnings` passes with
      zero warnings.
- [ ] Benchmarks are added where the use case affects performance (see
      `docs/TECHSTACK.md` performance budget).
- [ ] Documentation is updated (README, AGENTS.md, or the use case file —
      whichever is affected).
- [ ] No network calls were introduced.
- [ ] No secrets are logged, stored, or printed in debug output.
- [ ] The use case status is updated to `Ready for Review` in
      `USECASES.md`.

PRs that don't meet this list will be sent back, not partially merged.

---

## Engineering rules (non-negotiable)

These come from `docs/TECHSTACK.md` and apply to every PR:

- **Rust stable, Cargo workspace.** No JS/TS or Python in the product core.
- **No network access by default.** No telemetry, no analytics, no cloud
  calls of any kind — even opt-in ones without an explicit, separate use
  case and human approval.
- **No heavyweight database server, no external vector DB by default.**
- **Respect the performance budget.** If your change breaks the targets in
  `TECHSTACK.md` (CLI cold start, warm search p95, daemon idle RSS/CPU),
  the design needs to change — the budget doesn't move to fit the code.
- **Single writer path** for SQLite. Don't introduce a second write path
  "just for this feature."
- **Namespace isolation is a security boundary, not a convenience.** Any
  change touching search or storage must not leak memories across
  namespaces — this is tested explicitly and taken seriously.

---

## Pull request process

1. Comment on the relevant use case (or open one) before starting, so work
   isn't duplicated.
2. Branch name: `u-XXX-short-description` (e.g. `u-006-daemon-ipc`).
  One branch per use case: do not reuse a branch across multiple use cases.
3. Keep PRs scoped to one use case. Don't bundle unrelated fixes — open a
   separate PR.
4. Fill in the PR template (what use case, what scope items are covered,
   what's explicitly out of scope).
5. Move the use case to `Ready for Review` only once the Definition of Done
   checklist above is genuinely satisfied.
6. A human maintainer reviews and either requests changes or sets the status
   to `Approved` and merges.

---

## Reporting bugs / security issues

- **Regular bugs:** open a GitHub issue with repro steps, OS/platform, and
  `locus doctor` output if possible.
- **Security issues** (e.g. a way to leak data across namespaces, bypass
  secret redaction, or make the daemon listen on a public interface):
  do **not** open a public issue. See `SECURITY.md` for private disclosure
  instructions (or, if that file doesn't exist yet, contact the maintainers
  directly before filing anything public).

---

## A note for AI agent contributors

If you are an AI coding agent (Claude Code, Cursor, Cline, etc.) working in
this repo: read [`AGENTS.md`](AGENTS.md) in full before making changes. It is
not optional context — it defines what you're allowed to do autonomously
versus what requires human sign-off, and it is kept up to date specifically
so agents don't have to guess.

<!-- LOCUS:MEMORY_PROTOCOL:DOC:START -->
# Locus Memory Protocol

Locus is this project's long-term memory layer for AI coding agents.

## Before non-trivial changes

Consult Locus memory for project decisions, constraints, and preferences:

- **CLI**: run `locus context "<task>"` (returns a compressed Markdown brief or `NO_RELEVANT_MEMORY`)
- **MCP**: call `memory_search` with a short query about the area you are changing

## Recording decisions

If a new decision is confirmed, record it:

- **CLI**: `locus remember "<decision>" --type decision --namespace project:locus`
- **MCP**: `memory_save`

## Rules

- **Do not store secrets** — never save API keys, passwords, tokens, private credentials, or `.env` values in Locus.
- If `NO_RELEVANT_MEMORY` is returned, continue normally.
<!-- LOCUS:MEMORY_PROTOCOL:DOC:END -->
