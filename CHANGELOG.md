# Changelog

All notable changes to Locus are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.1] - 2026-08-24

### Changed

- Unquoted multi-term search uses FTS5 `OR` plus a coverage floor instead of
  requiring every token in the same memory (D-15). CLI hits and context-brief
  bullets show how much of the query matched (for example `86%`).
- Coverage includes memory type, so `tts decision` matches a `type=decision`
  memory that never says the word "decision", still inside the requested
  namespace.
- Question-shaped MCP, CLI, and hook queries drop stopwords and wrapping
  punctuation. `what was the local api decision?` is searched as `local`,
  `api`, `decision`.

### Fixed

- Long agent queries returned `NO_RELEVANT_MEMORY` because FTS5 `AND` and the
  LIKE fallback required the whole sentence to appear verbatim.
- Two-term queries such as `tts decision` dropped typed memories.
- Question queries flooded results by matching function words (`the`, `was`).

## [0.1.0] - 2026-08-18

### Added

- Local-first SQLite and FTS5 memory storage with namespace isolation,
  deterministic ranking, conflict detection, and secret redaction.
- `locus` CLI commands for saving, searching, contextual briefs, forgetting,
  graph visualization, diagnostics, hooks, initialization, and benchmarking.
- `locusd` single-writer daemon with local IPC and automatic lifecycle.
- MCP stdio server for memory search, save, forget, and status operations.
- Git post-commit ingestion and deterministic compaction-summary capture.
- Claude Code session and compaction lifecycle integration.
- Project initialization for Claude Code, Cursor, VS Code, Cline, and generic
  instruction-file fallbacks.
- Native visualization server and live graph launcher.
- Source installer, uninstaller, shell completions, Cargo packaging, and a
  Homebrew formula for all four shipped binaries.

### Security

- No network calls or telemetry in the memory and search paths.
- Write-time secret detection and redaction.
- Local namespace isolation and restrictive database permissions.

[0.1.1]: https://github.com/mustafakarakus/locus/releases/tag/v0.1.1
[0.1.0]: https://github.com/mustafakarakus/locus/releases/tag/v0.1.0