<!-- LOCUS:MEMORY_PROTOCOL:START -->
# Locus Memory Protocol

Locus is this project's long-term memory layer for AI coding agents.
Use it through the MCP tools: `memory_search`, `memory_save`, `memory_forget`, `memory_status`.

## Required behavior

1. **Before non-trivial code changes**, call `memory_search` with a short query about the area you are changing (identifiers, decisions, constraints).
2. **Follow** any decisions and constraints returned in the brief.
3. **If a new decision is confirmed** with the user, call `memory_save` (prefer type `decision` or `preference`).
4. **Do not save secrets** — never store API keys, passwords, tokens, private credentials, or `.env` values in Locus.
5. **If `NO_RELEVANT_MEMORY` is returned**, continue normally.

## Namespace

Prefer namespace `project:locus` for project-specific memories. Also consider `global` for cross-project preferences.

## Tools

- `memory_search` — returns a compressed Markdown brief (or `NO_RELEVANT_MEMORY`)
- `memory_save` — store a new memory
- `memory_forget` — delete a memory by id
- `memory_status` — daemon / database status
<!-- LOCUS:MEMORY_PROTOCOL:END -->
