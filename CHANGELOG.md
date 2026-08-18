# Changelog

All notable changes to Locus are documented in this file.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project uses [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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

[0.1.0]: https://github.com/mustafakarakus/locus/releases/tag/v0.1.0