//! SQLite-backed canonical memory store.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::conflict::{self, ConflictRecord};
use crate::context::{self, ContextBriefOptions};
use crate::ipc::protocol::Warning;
use crate::memory::{
    normalize_entities, normalize_namespace, normalize_optional, validate_content,
    validate_importance, validate_title, ListFilter, Memory, MemoryType, NewMemory, UpdateMemory,
};
use crate::search::{self, Fts5SearchEngine, Hit, Query, RankSignals, SearchEngine};
use crate::{Error, Result};

const DB_FILE_NAME: &str = "locus.db";

const PRAGMAS: &[&str] = &[
    "PRAGMA journal_mode = WAL;",
    "PRAGMA synchronous = NORMAL;",
    "PRAGMA temp_store = MEMORY;",
    "PRAGMA mmap_size = 268435456;",
    "PRAGMA cache_size = -20000;",
    "PRAGMA busy_timeout = 5000;",
    "PRAGMA foreign_keys = ON;",
];

const MIGRATIONS: &[(i64, &str, &str)] = &[
    (
        1,
        "initial_schema",
        "
CREATE TABLE IF NOT EXISTS memories (
    id TEXT PRIMARY KEY,
    namespace TEXT NOT NULL,
    type TEXT NOT NULL,
    title TEXT NOT NULL,
    content TEXT NOT NULL,
    importance INTEGER NOT NULL CHECK (importance >= 0 AND importance <= 100),
    source TEXT,
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS entities (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL UNIQUE
);

CREATE TABLE IF NOT EXISTS memory_entities (
    memory_id TEXT NOT NULL,
    entity_id INTEGER NOT NULL,
    PRIMARY KEY (memory_id, entity_id),
    FOREIGN KEY (memory_id) REFERENCES memories(id) ON DELETE CASCADE,
    FOREIGN KEY (entity_id) REFERENCES entities(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS migrations (
    version INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    applied_at INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_memories_namespace ON memories(namespace);
CREATE INDEX IF NOT EXISTS idx_memories_type ON memories(type);
CREATE INDEX IF NOT EXISTS idx_memories_updated_at ON memories(updated_at);
",
    ),
    (
        2,
        "fts5_search_index",
        "
CREATE VIRTUAL TABLE IF NOT EXISTS memory_fts USING fts5(
    memory_id UNINDEXED,
    title,
    content,
    entities,
    tokenize = 'unicode61 tokenchars ''_-/.:''',
    prefix = '2 3 4'
);

INSERT INTO memory_fts (memory_id, title, content, entities)
SELECT
    m.id,
    m.title,
    m.content,
    COALESCE((
        SELECT group_concat(e.name, ' ')
        FROM memory_entities me
        INNER JOIN entities e ON e.id = me.entity_id
        WHERE me.memory_id = m.id
    ), '')
FROM memories m
WHERE NOT EXISTS (
    SELECT 1 FROM memory_fts f WHERE f.memory_id = m.id
);
",
    ),
    (
        3,
        "conflict_tracking",
        "
CREATE TABLE IF NOT EXISTS memory_conflicts (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    memory_id_a TEXT NOT NULL,
    memory_id_b TEXT NOT NULL,
    reason TEXT NOT NULL,
    detected_at INTEGER NOT NULL,
    resolved_at INTEGER,
    FOREIGN KEY (memory_id_a) REFERENCES memories(id) ON DELETE CASCADE,
    FOREIGN KEY (memory_id_b) REFERENCES memories(id) ON DELETE CASCADE,
    UNIQUE (memory_id_a, memory_id_b)
);
CREATE INDEX IF NOT EXISTS idx_conflicts_memory_a ON memory_conflicts(memory_id_a);
CREATE INDEX IF NOT EXISTS idx_conflicts_memory_b ON memory_conflicts(memory_id_b);
",
    ),
    (
        4,
        "access_tracking",
        "
ALTER TABLE memories ADD COLUMN access_count INTEGER NOT NULL DEFAULT 0;
ALTER TABLE memories ADD COLUMN last_accessed_at INTEGER;
CREATE INDEX IF NOT EXISTS idx_memories_access_count ON memories(access_count);
CREATE INDEX IF NOT EXISTS idx_memories_last_accessed ON memories(last_accessed_at);
",
    ),
    (
        5,
        "fts_rowid_mapping",
        "
CREATE TABLE IF NOT EXISTS memory_fts_rowid (
    memory_id TEXT PRIMARY KEY,
    fts_rowid INTEGER NOT NULL,
    FOREIGN KEY (memory_id) REFERENCES memories(id) ON DELETE CASCADE
);

-- Drop orphaned FTS rows (no matching memories row) so the backfill below
-- never violates the foreign key; they are unreachable via search anyway.
DELETE FROM memory_fts
WHERE NOT EXISTS (SELECT 1 FROM memories m WHERE m.id = memory_fts.memory_id);

-- Backfill the mapping for FTS rows created before this migration so
-- delete/update paths can target the FTS rowid (O(log n)) instead of
-- scanning the whole FTS index on the UNINDEXED memory_id column.
INSERT OR IGNORE INTO memory_fts_rowid (memory_id, fts_rowid)
SELECT memory_id, rowid FROM memory_fts;
",
    ),
];

/// SQLite-backed storage for canonical memories.
#[derive(Debug, Clone)]
pub struct Store {
    db_path: PathBuf,
}

/// Outcome of [`Store::retrieve`]: the relevance-ranked hits plus the full
/// memory objects they refer to (used for access tracking and events).
#[derive(Debug, Clone)]
pub struct RetrieveOutcome {
    pub hits: Vec<Hit>,
    pub memories: Vec<Memory>,
}

impl Store {
    /// Opens (and initializes if needed) the default local Locus database.
    pub fn open_default() -> Result<Self> {
        let db_path = default_db_path()?;
        Self::open_at(db_path)
    }

    /// Opens (and initializes if needed) a database at a specific path.
    pub fn open_at<P: Into<PathBuf>>(path: P) -> Result<Self> {
        let db_path = path.into();
        ensure_parent_dir_restrictive(&db_path)?;

        {
            let conn = Connection::open(&db_path)?;
            apply_pragmas(&conn)?;
            run_migrations(&conn)?;
        }

        ensure_file_restrictive(&db_path)?;
        Ok(Self { db_path })
    }

    /// Runs schema migrations idempotently.
    pub fn run_migrations(&self) -> Result<()> {
        let conn = self.connect_rw()?;
        run_migrations(&conn)
    }

    /// Inserts a memory and returns the new memory id.
    pub fn insert_memory(&self, input: NewMemory) -> Result<String> {
        validate_title(&input.title)?;
        validate_content(&input.content)?;
        validate_importance(input.importance)?;

        let id = Uuid::new_v4().to_string();
        let namespace = normalize_namespace(input.namespace);
        let title = input.title.trim().to_string();
        let content = input.content.trim().to_string();
        let source = normalize_optional(input.source);
        let entities = normalize_entities(input.entities);
        let now = now_unix();

        let mut conn = self.connect_rw()?;
        let tx = conn.transaction()?;
        tx.execute(
            "
            INSERT INTO memories (
                id, namespace, type, title, content, importance, source, created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            ",
            params![
                id,
                namespace,
                input.memory_type.as_str(),
                title,
                content,
                i64::from(input.importance),
                source,
                now,
                now
            ],
        )?;

        link_entities(&tx, &id, entities)?;
        upsert_fts_row(&tx, &id, &title, &content)?;
        tx.commit()?;
        Ok(id)
    }

    /// Inserts a memory with redact-or-warn secret handling (U-011).
    ///
    /// Detected secrets in the title/content are replaced with a
    /// `[REDACTED:rule-id]` placeholder before storage and a non-fatal warning
    /// is returned. When `allow_secret` is set the memory is stored verbatim
    /// (explicit user consent) with no warnings. Nothing is ever hard-rejected.
    pub fn insert_memory_checked(
        &self,
        input: NewMemory,
        allow_secret: bool,
    ) -> Result<(String, Vec<Warning>)> {
        let (redacted_title, redacted_content, matches) =
            crate::security::redact_title_and_content(&input.title, &input.content);

        if matches.is_empty() || allow_secret {
            let id = self.insert_memory(input)?;
            return Ok((id, Vec::new()));
        }

        let input = NewMemory {
            title: redacted_title,
            content: redacted_content,
            ..input
        };
        let id = self.insert_memory(input)?;
        Ok((id, crate::security::build_warnings(&matches)))
    }

    /// Updates an existing memory.
    pub fn update_memory(&self, input: UpdateMemory) -> Result<()> {
        if input.id.trim().is_empty() {
            return Err(Error::InvalidInput("id must not be empty".to_string()));
        }
        validate_title(&input.title)?;
        validate_content(&input.content)?;
        validate_importance(input.importance)?;

        let namespace = normalize_namespace(input.namespace);
        let title = input.title.trim().to_string();
        let content = input.content.trim().to_string();
        let source = normalize_optional(input.source);
        let entities = normalize_entities(input.entities);
        let now = now_unix();

        let mut conn = self.connect_rw()?;
        let tx = conn.transaction()?;
        let affected = tx.execute(
            "
            UPDATE memories
            SET namespace = ?, type = ?, title = ?, content = ?, importance = ?, source = ?, updated_at = ?
            WHERE id = ?
            ",
            params![
                namespace,
                input.memory_type.as_str(),
                title,
                content,
                i64::from(input.importance),
                source,
                now,
                input.id
            ],
        )?;

        if affected == 0 {
            return Err(Error::NotFound("memory not found".to_string()));
        }

        tx.execute(
            "DELETE FROM memory_entities WHERE memory_id = ?",
            params![input.id],
        )?;
        link_entities(&tx, &input.id, entities)?;
        upsert_fts_row(&tx, &input.id, &title, &content)?;
        tx.commit()?;
        Ok(())
    }

    /// Deletes a memory by id.
    pub fn delete_memory(&self, id: &str) -> Result<()> {
        if id.trim().is_empty() {
            return Err(Error::InvalidInput("id must not be empty".to_string()));
        }

        let mut conn = self.connect_rw()?;
        let tx = conn.transaction()?;

        // Delete the FTS row by rowid first (O(log n) via the mapping), then
        // the memory row. The FK cascade cleans up the mapping row itself.
        let fts_rowid: Option<i64> = tx
            .query_row(
                "SELECT fts_rowid FROM memory_fts_rowid WHERE memory_id = ?",
                params![id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(rowid) = fts_rowid {
            tx.execute("DELETE FROM memory_fts WHERE rowid = ?", params![rowid])?;
        }

        let affected = tx.execute("DELETE FROM memories WHERE id = ?", params![id])?;
        if affected == 0 {
            return Err(Error::NotFound("memory not found".to_string()));
        }

        tx.commit()?;
        Ok(())
    }

    /// Searches memories using the default FTS5 backend and shared reranking.
    pub fn search(&self, query: Query) -> Result<Vec<Hit>> {
        query.validate()?;

        let engine = Fts5SearchEngine::open_at(self.db_path.clone());
        let hits = engine.search(&query)?;
        let signals = self.load_rank_signals_for_hits(&hits)?;

        Ok(search::rerank_hits(hits, &signals))
    }

    /// Retrieves memories surfaced by a search query, returning both the
    /// relevance-ranked hits and the full memory objects they refer to.
    ///
    /// This is the single "surface to a caller" funnel used by the daemon's
    /// search and context handlers so access tracking (U-016) hooks here.
    pub fn retrieve(&self, query: Query) -> Result<RetrieveOutcome> {
        query.validate()?;
        let hits = self.search(query)?;
        let memories = self.load_memories_for_hits(&hits)?;
        Ok(RetrieveOutcome { hits, memories })
    }

    /// Builds a compressed markdown context brief from search results.
    pub fn context_brief(&self, query: Query, options: ContextBriefOptions) -> Result<String> {
        Ok(self.context_brief_with_memories(query, options)?.0)
    }

    /// Builds a context brief and returns the memories that made it into the
    /// final output, so the daemon can record access and emit live events for
    /// exactly what was surfaced (U-016).
    pub fn context_brief_with_memories(
        &self,
        query: Query,
        options: ContextBriefOptions,
    ) -> Result<(String, Vec<Memory>)> {
        let outcome = self.retrieve(query)?;
        if outcome.hits.is_empty() {
            return Ok((context::NO_RELEVANT_MEMORY.to_string(), Vec::new()));
        }
        let (brief, selected) =
            context::build_context_brief_with_selected(&outcome.memories, options);
        Ok((brief, selected.into_iter().cloned().collect()))
    }

    /// Records access to memories (U-016 access tracking).
    ///
    /// A batched, cheap update that increments `access_count` and stamps
    /// `last_accessed_at` for every id. Must run on the single-writer path (the
    /// daemon fires this through the writer thread fire-and-forget); it is never
    /// called on the latency-critical search response path.
    pub fn record_access(&self, ids: &[String]) -> Result<()> {
        if ids.is_empty() {
            return Ok(());
        }

        let conn = self.connect_rw()?;
        let mut stmt = conn.prepare(
            "
            UPDATE memories
            SET access_count = access_count + 1, last_accessed_at = ?
            WHERE id = ?
            ",
        )?;
        let now = now_unix();
        for id in ids {
            stmt.execute(params![now, id])?;
        }
        Ok(())
    }

    /// Builds a namespace-scoped summary brief without a user query.
    ///
    /// Used by the default-query strategy for session-start hook injection
    /// (U-015). Always scopes to a concrete namespace — `None` maps to
    /// `global` — so a summary never leaks across namespaces. Uses the same
    /// shared [`context::build_context_brief`] engine as query-driven briefs.
    pub fn summary_brief(
        &self,
        namespace: Option<&str>,
        options: ContextBriefOptions,
    ) -> Result<String> {
        let namespace = namespace
            .map(str::trim)
            .filter(|ns| !ns.is_empty())
            .unwrap_or("global");
        let filter = ListFilter {
            namespace: Some(namespace.to_string()),
            memory_type: None,
            limit: Some(20),
        };
        let memories = self.list_memories(filter)?;
        Ok(context::build_context_brief(&memories, options))
    }

    /// Fetches a memory by id.
    pub fn get_memory_by_id(&self, id: &str) -> Result<Memory> {
        if id.trim().is_empty() {
            return Err(Error::InvalidInput("id must not be empty".to_string()));
        }

        let conn = self.connect_ro()?;
        let row = conn
            .query_row(
                "
                SELECT id, namespace, type, title, content, importance, source, created_at, updated_at,
                       access_count, last_accessed_at
                FROM memories
                WHERE id = ?
                ",
                params![id],
                row_to_memory_base,
            )
            .optional()?;

        let mut memory = match row {
            Some(item) => item,
            None => return Err(Error::NotFound("memory not found".to_string())),
        };
        memory.entities = load_entities(&conn, &memory.id)?;
        Ok(memory)
    }

    /// Lists memories with optional namespace/type filtering.
    pub fn list_memories(&self, filter: ListFilter) -> Result<Vec<Memory>> {
        let conn = self.connect_ro()?;
        let mut sql = String::from(
            "
            SELECT id, namespace, type, title, content, importance, source, created_at, updated_at,
                   access_count, last_accessed_at
            FROM memories
            ",
        );

        let mut predicates = Vec::new();
        let mut dynamic_values = Vec::<String>::new();

        if let Some(namespace) = filter.namespace.and_then(|ns| {
            let trimmed = ns.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }) {
            predicates.push("namespace = ?");
            dynamic_values.push(namespace);
        }

        if let Some(memory_type) = filter.memory_type {
            predicates.push("type = ?");
            dynamic_values.push(memory_type.as_str().to_string());
        }

        if !predicates.is_empty() {
            sql.push_str(" WHERE ");
            sql.push_str(&predicates.join(" AND "));
        }

        sql.push_str(" ORDER BY updated_at DESC");

        if let Some(limit) = filter.limit {
            sql.push_str(&format!(" LIMIT {limit}"));
        }

        let mut statement = conn.prepare(&sql)?;
        let values: Vec<&str> = dynamic_values.iter().map(String::as_str).collect();
        let mut rows = statement.query(params_from_iter(values))?;

        let mut memories = Vec::new();
        while let Some(row) = rows.next()? {
            let mut memory = row_to_memory_base(row)?;
            memory.entities = load_entities(&conn, &memory.id)?;
            memories.push(memory);
        }

        Ok(memories)
    }

    /// Returns the path to the underlying SQLite database file.
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// Returns the number of canonical memories stored.
    pub fn memory_count(&self) -> Result<usize> {
        let conn = self.connect_ro()?;
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))?;
        Ok(usize::try_from(count).unwrap_or(0))
    }

    /// Returns the number of rows currently indexed in the FTS5 table.
    pub fn fts_row_count(&self) -> Result<usize> {
        let conn = self.connect_ro()?;
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM memory_fts", [], |row| row.get(0))?;
        Ok(usize::try_from(count).unwrap_or(0))
    }

    /// Reports whether the FTS5 index has drifted from the canonical rows.
    ///
    /// This is a cheap consistency check (comparing row counts) used on daemon
    /// startup to decide whether a `reindex` is warranted. It never mutates
    /// data.
    pub fn fts_out_of_sync(&self) -> Result<bool> {
        Ok(self.memory_count()? != self.fts_row_count()?)
    }

    /// Rebuilds the FTS5 search table from the canonical memory rows.
    ///
    /// For the default FTS5 backend this is a consistency-repair operation on
    /// the same database file: it clears `memory_fts` and repopulates it from
    /// `memories`/`entities`. It never deletes canonical data and returns the
    /// number of rows indexed.
    pub fn reindex(&self) -> Result<usize> {
        let mut conn = self.connect_rw()?;
        let tx = conn.transaction()?;
        tx.execute("DELETE FROM memory_fts", [])?;
        tx.execute(
            "
            INSERT INTO memory_fts (memory_id, title, content, entities)
            SELECT
                m.id,
                m.title,
                m.content,
                COALESCE((
                    SELECT group_concat(e.name, ' ')
                    FROM memory_entities me
                    INNER JOIN entities e ON e.id = me.entity_id
                    WHERE me.memory_id = m.id
                ), '')
            FROM memories m
            ",
            [],
        )?;
        let count: i64 = tx.query_row("SELECT COUNT(*) FROM memory_fts", [], |row| row.get(0))?;
        tx.commit()?;
        Ok(usize::try_from(count).unwrap_or(0))
    }

    /// Detects conflicts between `memory` and existing memories, persisting any
    /// new conflict records found.
    ///
    /// Only types in [`conflict::CONFLICT_ELIGIBLE_TYPES`] are examined. Two
    /// memories are treated as a potential conflict when they share the same
    /// namespace and type and their titles overlap on at least
    /// [`conflict::MIN_SHARED_WORDS`] significant keywords.
    ///
    /// Errors are returned but are treated as best-effort by callers (e.g. the
    /// writer thread) — a failure here must never corrupt or lose canonical
    /// memory data.
    pub fn detect_and_store_conflicts(&self, memory: &Memory) -> Result<()> {
        if !conflict::is_conflict_eligible(memory.memory_type) {
            return Ok(());
        }

        let words = conflict::significant_words(&memory.title);
        if words.len() < conflict::MIN_SHARED_WORDS {
            return Ok(());
        }

        // Collect candidates from the same namespace + type using a RO connection.
        let candidates: Vec<(String, String)> = {
            let conn = self.connect_ro()?;
            let mut stmt = conn.prepare(
                "SELECT id, title FROM memories WHERE namespace = ? AND type = ? AND id != ?",
            )?;
            let result = stmt
                .query_map(
                    params![memory.namespace, memory.memory_type.as_str(), memory.id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            result
        };

        if candidates.is_empty() {
            return Ok(());
        }

        let now = now_unix();
        let conn = self.connect_rw()?;

        for (other_id, other_title) in candidates {
            let shared_words: Vec<String> = words
                .iter()
                .filter(|w| conflict::significant_words(&other_title).contains(w))
                .cloned()
                .collect();

            if shared_words.len() < conflict::MIN_SHARED_WORDS {
                continue;
            }

            // Canonical ordering: lexicographically earlier id is always `a`.
            let (id_a, id_b) = if memory.id < other_id {
                (memory.id.as_str(), other_id.as_str())
            } else {
                (other_id.as_str(), memory.id.as_str())
            };

            let reason = format!(
                "similar titles: shared keywords [{}]",
                shared_words.join(", ")
            );

            conn.execute(
                "INSERT OR IGNORE INTO memory_conflicts
                 (memory_id_a, memory_id_b, reason, detected_at)
                 VALUES (?, ?, ?, ?)",
                params![id_a, id_b, reason, now],
            )?;
        }

        Ok(())
    }

    /// Lists conflict records, optionally filtered to a single namespace.
    ///
    /// When `namespace` is provided the filter is applied to `memory_id_a`'s
    /// namespace. Results are ordered newest-detected first.
    pub fn list_conflicts(&self, namespace: Option<String>) -> Result<Vec<ConflictRecord>> {
        let conn = self.connect_ro()?;

        if let Some(ns) = namespace {
            let ns = normalize_namespace(Some(ns));
            let mut stmt = conn.prepare(
                "SELECT mc.id, mc.memory_id_a, mc.memory_id_b, mc.reason,
                        mc.detected_at, mc.resolved_at
                 FROM memory_conflicts mc
                 INNER JOIN memories m ON m.id = mc.memory_id_a
                 WHERE m.namespace = ?
                 ORDER BY mc.detected_at DESC",
            )?;
            let records = stmt
                .query_map(params![ns], row_to_conflict)?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(records)
        } else {
            let mut stmt = conn.prepare(
                "SELECT id, memory_id_a, memory_id_b, reason, detected_at, resolved_at
                 FROM memory_conflicts
                 ORDER BY detected_at DESC",
            )?;
            let records = stmt
                .query_map([], row_to_conflict)?
                .collect::<std::result::Result<Vec<_>, _>>()?;
            Ok(records)
        }
    }

    /// Returns the number of currently unresolved conflict records.
    pub fn conflict_count(&self) -> Result<usize> {
        let conn = self.connect_ro()?;
        let count: i64 = conn.query_row(
            "SELECT COUNT(*) FROM memory_conflicts WHERE resolved_at IS NULL",
            [],
            |row| row.get(0),
        )?;
        Ok(usize::try_from(count).unwrap_or(0))
    }

    fn connect_rw(&self) -> Result<Connection> {
        let conn = Connection::open(&self.db_path)?;
        apply_pragmas(&conn)?;
        Ok(conn)
    }

    pub(crate) fn connect_ro(&self) -> Result<Connection> {
        let conn = Connection::open_with_flags(
            &self.db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )?;
        apply_pragmas(&conn)?;
        Ok(conn)
    }

    fn load_rank_signals_for_hits(&self, hits: &[Hit]) -> Result<HashMap<String, RankSignals>> {
        if hits.is_empty() {
            return Ok(HashMap::new());
        }

        let conn = self.connect_ro()?;
        let mut signals = HashMap::with_capacity(hits.len());

        let mut stmt = conn.prepare(
            "
            SELECT importance, updated_at
            FROM memories
            WHERE id = ?
            ",
        )?;

        for hit in hits {
            let maybe = stmt
                .query_row(params![hit.id], |row| {
                    let importance_i64: i64 = row.get(0)?;
                    let importance =
                        u8::try_from(importance_i64).map_err(|_| rusqlite::Error::InvalidQuery)?;
                    let updated_at: i64 = row.get(1)?;
                    Ok(RankSignals {
                        importance,
                        updated_at,
                    })
                })
                .optional()?;

            if let Some(value) = maybe {
                signals.insert(hit.id.clone(), value);
            }
        }

        Ok(signals)
    }

    fn load_memories_for_hits(&self, hits: &[Hit]) -> Result<Vec<Memory>> {
        if hits.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.connect_ro()?;
        let mut stmt = conn.prepare(
            "
            SELECT id, namespace, type, title, content, importance, source, created_at, updated_at,
                   access_count, last_accessed_at
            FROM memories
            WHERE id = ?
            ",
        )?;

        let mut memories = Vec::with_capacity(hits.len());
        for hit in hits {
            let row = stmt
                .query_row(params![hit.id], row_to_memory_base)
                .optional()?;

            if let Some(mut memory) = row {
                memory.entities = load_entities(&conn, &memory.id)?;
                memories.push(memory);
            }
        }

        Ok(memories)
    }
}

fn default_db_path() -> Result<PathBuf> {
    let home = std::env::var("HOME")
        .map(PathBuf::from)
        .map_err(|_| Error::Other("HOME environment variable is not set".to_string()))?;
    Ok(home.join(".locus").join(DB_FILE_NAME))
}

fn run_migrations(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS migrations (
            version INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            applied_at INTEGER NOT NULL
        );
        ",
    )?;

    let mut stmt = conn.prepare("SELECT version FROM migrations")?;
    let existing = stmt
        .query_map([], |row| row.get::<_, i64>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;

    for (version, name, sql) in MIGRATIONS {
        if existing
            .iter()
            .any(|existing_version| existing_version == version)
        {
            continue;
        }

        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(sql)?;
        tx.execute(
            "INSERT INTO migrations (version, name, applied_at) VALUES (?, ?, ?)",
            params![version, name, now_unix()],
        )?;
        tx.commit()?;
    }

    Ok(())
}

pub(crate) fn apply_pragmas(conn: &Connection) -> Result<()> {
    for pragma in PRAGMAS {
        conn.execute_batch(pragma)?;
    }
    Ok(())
}

fn link_entities(
    tx: &rusqlite::Transaction<'_>,
    memory_id: &str,
    entities: Vec<String>,
) -> Result<()> {
    for entity in entities {
        tx.execute(
            "INSERT INTO entities (name) VALUES (?) ON CONFLICT(name) DO NOTHING",
            params![entity],
        )?;

        let entity_id: i64 = tx.query_row(
            "SELECT id FROM entities WHERE name = ?",
            params![entity],
            |row| row.get(0),
        )?;

        tx.execute(
            "
            INSERT INTO memory_entities (memory_id, entity_id)
            VALUES (?, ?)
            ON CONFLICT(memory_id, entity_id) DO NOTHING
            ",
            params![memory_id, entity_id],
        )?;
    }
    Ok(())
}

fn upsert_fts_row(
    tx: &rusqlite::Transaction<'_>,
    memory_id: &str,
    title: &str,
    content: &str,
) -> Result<()> {
    let entities: String = tx.query_row(
        "
        SELECT COALESCE(group_concat(e.name, ' '), '')
        FROM memory_entities me
        INNER JOIN entities e ON e.id = me.entity_id
        WHERE me.memory_id = ?
        ",
        params![memory_id],
        |row| row.get(0),
    )?;

    // Delete any existing FTS row by rowid. `memory_id` is UNINDEXED in the
    // FTS table, so a `WHERE memory_id = ?` delete would scan the entire FTS
    // index (O(n) per save). The memory_fts_rowid mapping turns this into an
    // O(log n) rowid lookup + delete.
    let existing_rowid: Option<i64> = tx
        .query_row(
            "SELECT fts_rowid FROM memory_fts_rowid WHERE memory_id = ?",
            params![memory_id],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(rowid) = existing_rowid {
        tx.execute("DELETE FROM memory_fts WHERE rowid = ?", params![rowid])?;
        tx.execute(
            "DELETE FROM memory_fts_rowid WHERE memory_id = ?",
            params![memory_id],
        )?;
    }

    tx.execute(
        "
        INSERT INTO memory_fts (memory_id, title, content, entities)
        VALUES (?, ?, ?, ?)
        ",
        params![memory_id, title, content, entities],
    )?;
    let new_rowid = tx.last_insert_rowid();
    tx.execute(
        "INSERT INTO memory_fts_rowid (memory_id, fts_rowid) VALUES (?, ?)",
        params![memory_id, new_rowid],
    )?;
    Ok(())
}

pub(crate) fn entities_of(conn: &Connection, memory_id: &str) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "
        SELECT e.name
        FROM entities e
        INNER JOIN memory_entities me ON me.entity_id = e.id
        WHERE me.memory_id = ?
        ORDER BY e.name ASC
        ",
    )?;

    let rows = stmt
        .query_map(params![memory_id], |row| row.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

fn load_entities(conn: &Connection, memory_id: &str) -> Result<Vec<String>> {
    entities_of(conn, memory_id)
}

fn row_to_conflict(
    row: &rusqlite::Row<'_>,
) -> std::result::Result<ConflictRecord, rusqlite::Error> {
    Ok(ConflictRecord {
        id: row.get(0)?,
        memory_id_a: row.get(1)?,
        memory_id_b: row.get(2)?,
        reason: row.get(3)?,
        detected_at: row.get(4)?,
        resolved_at: row.get(5)?,
    })
}

fn row_to_memory_base(row: &rusqlite::Row<'_>) -> std::result::Result<Memory, rusqlite::Error> {
    let type_raw: String = row.get(2)?;
    let memory_type = MemoryType::parse(&type_raw).map_err(|_| rusqlite::Error::InvalidQuery)?;

    let importance_i64: i64 = row.get(5)?;
    let importance = u8::try_from(importance_i64).map_err(|_| rusqlite::Error::InvalidQuery)?;

    Ok(Memory {
        id: row.get(0)?,
        namespace: row.get(1)?,
        memory_type,
        title: row.get(3)?,
        content: row.get(4)?,
        entities: Vec::new(),
        importance,
        source: row.get(6)?,
        created_at: row.get(7)?,
        updated_at: row.get(8)?,
        access_count: row.get(9)?,
        last_accessed_at: row.get(10)?,
    })
}

fn now_unix() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}

fn ensure_parent_dir_restrictive(path: &Path) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| Error::Other("database path has no parent directory".to_string()))?;
    fs::create_dir_all(parent)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }

    Ok(())
}

fn ensure_file_restrictive(path: &Path) -> Result<()> {
    if !path.exists() {
        return Err(Error::Other("database file was not created".to_string()));
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;

    fn test_store() -> (Store, TempDir, PathBuf) {
        let tmp = TempDir::new().expect("temp dir");
        let db_path = tmp.path().join("locus.db");
        let store = Store::open_at(db_path.clone()).expect("store should initialize");
        (store, tmp, db_path)
    }

    fn sample_new_memory(namespace: Option<&str>, memory_type: MemoryType) -> NewMemory {
        NewMemory {
            namespace: namespace.map(str::to_string),
            memory_type,
            title: "Use Postgres for auth".to_string(),
            content: "Auth service persists users in Postgres".to_string(),
            entities: vec!["postgres".to_string(), "auth".to_string()],
            importance: 80,
            source: Some("manual".to_string()),
        }
    }

    #[test]
    fn database_initialization_is_idempotent() {
        let tmp = TempDir::new().expect("temp dir");
        let db_path = tmp.path().join("locus.db");

        let first = Store::open_at(db_path.clone());
        let second = Store::open_at(db_path);

        assert!(first.is_ok());
        assert!(second.is_ok());
    }

    #[test]
    fn migration_runner_can_apply_migrations_twice_safely() {
        let (store, _tmp, _) = test_store();
        assert!(store.run_migrations().is_ok());
        assert!(store.run_migrations().is_ok());
    }

    #[test]
    fn insert_memory_works() {
        let (store, _tmp, _) = test_store();
        let id = store
            .insert_memory(sample_new_memory(
                Some("project:auth"),
                MemoryType::Decision,
            ))
            .expect("insert should work");

        let inserted = store.get_memory_by_id(&id).expect("memory should exist");
        assert_eq!(inserted.namespace, "project:auth");
        assert_eq!(inserted.memory_type, MemoryType::Decision);
        assert_eq!(inserted.importance, 80);
    }

    #[test]
    fn invalid_memory_type_is_rejected() {
        let (store, _tmp, db_path) = test_store();
        let conn = Connection::open(db_path).expect("open db");
        conn.execute(
            "
            INSERT INTO memories (id, namespace, type, title, content, importance, source, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
            ",
            params![
                "forced-invalid-type",
                "project:auth",
                "not-a-valid-type",
                "Invalid type row",
                "Injected for test",
                10_i64,
                Option::<String>::None,
                now_unix(),
                now_unix(),
            ],
        )
        .expect("insert raw row");

        let err = store
            .list_memories(ListFilter::default())
            .expect_err("invalid type should error while reading");
        assert!(matches!(err, Error::Sql(_)));
    }

    #[test]
    fn missing_namespace_defaults_to_global() {
        let (store, _tmp, _) = test_store();
        let id = store
            .insert_memory(sample_new_memory(None, MemoryType::Fact))
            .expect("insert should work");

        let inserted = store.get_memory_by_id(&id).expect("memory should exist");
        assert_eq!(inserted.namespace, "global");
    }

    #[test]
    fn delete_memory_works() {
        let (store, _tmp, _) = test_store();
        let id = store
            .insert_memory(sample_new_memory(Some("project:auth"), MemoryType::Task))
            .expect("insert should work");

        store.delete_memory(&id).expect("delete should work");
        let err = store
            .get_memory_by_id(&id)
            .expect_err("memory should be gone after delete");
        assert!(matches!(err, Error::NotFound(_)));
    }

    #[test]
    fn namespace_isolation_works() {
        let (store, _tmp, _) = test_store();
        store
            .insert_memory(sample_new_memory(
                Some("project:auth"),
                MemoryType::Decision,
            ))
            .expect("insert first");
        store
            .insert_memory(sample_new_memory(
                Some("project:billing"),
                MemoryType::Decision,
            ))
            .expect("insert second");

        let auth = store
            .list_memories(ListFilter {
                namespace: Some("project:auth".to_string()),
                memory_type: None,
                limit: None,
            })
            .expect("query auth namespace");

        assert_eq!(auth.len(), 1);
        assert_eq!(auth[0].namespace, "project:auth");
    }

    #[test]
    fn exact_keyword_search_works() {
        let (store, _tmp, _) = test_store();
        store
            .insert_memory(sample_new_memory(
                Some("project:auth"),
                MemoryType::Decision,
            ))
            .expect("insert memory");

        let mut query = Query::new("postgres");
        query.namespace = Some("project:auth".to_string());
        let hits = store.search(query).expect("search should succeed");

        assert!(!hits.is_empty());
    }

    #[test]
    fn phrase_search_works() {
        let (store, _tmp, _) = test_store();
        store
            .insert_memory(NewMemory {
                namespace: Some("project:auth".to_string()),
                memory_type: MemoryType::Code,
                title: "Auth middleware".to_string(),
                content: "Use auth service middleware for token verification".to_string(),
                entities: vec!["AuthService::verify_token".to_string()],
                importance: 65,
                source: None,
            })
            .expect("insert memory");

        let mut query = Query::new("\"token verification\"");
        query.namespace = Some("project:auth".to_string());
        let hits = store.search(query).expect("phrase search should succeed");

        assert!(!hits.is_empty());
    }

    #[test]
    fn prefix_search_works() {
        let (store, _tmp, _) = test_store();
        store
            .insert_memory(sample_new_memory(
                Some("project:auth"),
                MemoryType::Decision,
            ))
            .expect("insert memory");

        let mut query = Query::new("post*");
        query.namespace = Some("project:auth".to_string());
        let hits = store.search(query).expect("prefix search should succeed");

        assert!(!hits.is_empty());
    }

    #[test]
    fn search_namespace_filter_prevents_leakage() {
        let (store, _tmp, _) = test_store();
        let auth_id = store
            .insert_memory(NewMemory {
                namespace: Some("project:auth".to_string()),
                memory_type: MemoryType::Decision,
                title: "Auth DB".to_string(),
                content: "Use Postgres in auth service".to_string(),
                entities: vec![],
                importance: 50,
                source: None,
            })
            .expect("insert auth");

        store
            .insert_memory(NewMemory {
                namespace: Some("project:billing".to_string()),
                memory_type: MemoryType::Decision,
                title: "Billing DB".to_string(),
                content: "Use Postgres in billing service".to_string(),
                entities: vec![],
                importance: 50,
                source: None,
            })
            .expect("insert billing");

        let mut query = Query::new("postgres");
        query.namespace = Some("project:auth".to_string());
        let hits = store.search(query).expect("search should succeed");

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, auth_id);
    }

    #[test]
    fn search_type_filter_works() {
        let (store, _tmp, _) = test_store();
        store
            .insert_memory(NewMemory {
                namespace: Some("project:auth".to_string()),
                memory_type: MemoryType::Decision,
                title: "DB decision".to_string(),
                content: "Postgres decision".to_string(),
                entities: vec![],
                importance: 50,
                source: None,
            })
            .expect("insert decision");

        let code_id = store
            .insert_memory(NewMemory {
                namespace: Some("project:auth".to_string()),
                memory_type: MemoryType::Code,
                title: "DB code path".to_string(),
                content: "Postgres connection pool code".to_string(),
                entities: vec![],
                importance: 50,
                source: None,
            })
            .expect("insert code");

        let mut query = Query::new("postgres");
        query.namespace = Some("project:auth".to_string());
        query.memory_type = Some(MemoryType::Code);
        let hits = store.search(query).expect("search should succeed");

        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].id, code_id);
    }

    #[test]
    fn identifier_search_works() {
        let (store, _tmp, _) = test_store();
        let id = store
            .insert_memory(NewMemory {
                namespace: Some("project:auth".to_string()),
                memory_type: MemoryType::Code,
                title: "Token verifier".to_string(),
                content: "Use AuthService::verify_token in auth/router.rs".to_string(),
                entities: vec![
                    "AuthService::verify_token".to_string(),
                    "auth/router.rs".to_string(),
                ],
                importance: 75,
                source: None,
            })
            .expect("insert code memory");

        let mut query = Query::new("AuthService::verify_token");
        query.namespace = Some("project:auth".to_string());
        let hits = store
            .search(query)
            .expect("identifier search should succeed");

        assert!(!hits.is_empty());
        assert_eq!(hits[0].id, id);
    }

    #[test]
    fn partial_name_search_works() {
        let (store, _tmp, _) = test_store();
        store
            .insert_memory(NewMemory {
                namespace: Some("project:auth".to_string()),
                memory_type: MemoryType::Code,
                title: "Verifier function".to_string(),
                content: "Call verify_token_handler from auth API route".to_string(),
                entities: vec!["verify_token_handler".to_string()],
                importance: 40,
                source: None,
            })
            .expect("insert memory");

        let mut query = Query::new("fy_token_han");
        query.namespace = Some("project:auth".to_string());
        let hits = store.search(query).expect("partial search should succeed");

        assert!(!hits.is_empty());
    }

    #[test]
    fn typo_tolerance_behavior_is_measured_and_stable() {
        let (store, _tmp, _) = test_store();
        store
            .insert_memory(sample_new_memory(
                Some("project:auth"),
                MemoryType::Decision,
            ))
            .expect("insert memory");

        let mut exact = Query::new("postgres");
        exact.namespace = Some("project:auth".to_string());
        let exact_hits = store.search(exact).expect("exact search should succeed");

        let mut typo = Query::new("postgrez");
        typo.namespace = Some("project:auth".to_string());
        let typo_hits = store.search(typo).expect("typo search should execute");

        // Documented behavior for FTS5 default backend: typo queries may return
        // fewer results than exact lexical matches.
        assert!(exact_hits.len() >= typo_hits.len());
    }

    #[test]
    fn reranker_boosts_newer_and_higher_importance_for_close_matches() {
        let (store, _tmp, db_path) = test_store();
        let older_id = store
            .insert_memory(NewMemory {
                namespace: Some("project:auth".to_string()),
                memory_type: MemoryType::Decision,
                title: "Routing strategy".to_string(),
                content: "Use middleware for token verification".to_string(),
                entities: vec!["verify_token".to_string()],
                importance: 20,
                source: None,
            })
            .expect("insert older");

        let newer_id = store
            .insert_memory(NewMemory {
                namespace: Some("project:auth".to_string()),
                memory_type: MemoryType::Decision,
                title: "Routing strategy v2".to_string(),
                content: "Use middleware for token verification".to_string(),
                entities: vec!["verify_token".to_string()],
                importance: 95,
                source: None,
            })
            .expect("insert newer");

        let conn = Connection::open(db_path).expect("open db");
        conn.execute(
            "UPDATE memories SET updated_at = ? WHERE id = ?",
            params![1_000_i64, older_id],
        )
        .expect("set older ts");
        conn.execute(
            "UPDATE memories SET updated_at = ? WHERE id = ?",
            params![2_000_i64, newer_id.clone()],
        )
        .expect("set newer ts");

        let mut query = Query::new("token verification");
        query.namespace = Some("project:auth".to_string());
        let hits = store.search(query).expect("search should succeed");

        assert!(!hits.is_empty());
        assert_eq!(hits[0].id, newer_id);
    }

    #[test]
    fn fts_table_stays_consistent_after_insert_update_delete() {
        let (store, _tmp, _) = test_store();
        let id = store
            .insert_memory(NewMemory {
                namespace: Some("project:auth".to_string()),
                memory_type: MemoryType::Code,
                title: "Use auth guard".to_string(),
                content: "auth_guard handles all auth routes".to_string(),
                entities: vec!["auth_guard".to_string()],
                importance: 60,
                source: None,
            })
            .expect("insert memory");

        let mut query = Query::new("auth_guard");
        query.namespace = Some("project:auth".to_string());
        let inserted_hits = store.search(query.clone()).expect("search after insert");
        assert_eq!(inserted_hits.len(), 1);

        store
            .update_memory(UpdateMemory {
                id: id.clone(),
                namespace: Some("project:auth".to_string()),
                memory_type: MemoryType::Code,
                title: "Use auth gate".to_string(),
                content: "auth_gate handles all auth routes".to_string(),
                entities: vec!["auth_gate".to_string()],
                importance: 60,
                source: None,
            })
            .expect("update memory");

        let mut old_query = Query::new("auth_guard");
        old_query.namespace = Some("project:auth".to_string());
        let old_hits = store.search(old_query).expect("search old token");
        assert!(old_hits.is_empty());

        let mut new_query = Query::new("auth_gate");
        new_query.namespace = Some("project:auth".to_string());
        let new_hits = store.search(new_query).expect("search new token");
        assert_eq!(new_hits.len(), 1);

        store.delete_memory(&id).expect("delete memory");
        let mut deleted_query = Query::new("auth_gate");
        deleted_query.namespace = Some("project:auth".to_string());
        let deleted_hits = store.search(deleted_query).expect("search deleted token");
        assert!(deleted_hits.is_empty());
    }

    #[test]
    fn empty_search_returns_no_relevant_memory() {
        let (store, _tmp, _) = test_store();
        let mut query = Query::new("nothing-here");
        query.namespace = Some("project:auth".to_string());

        let brief = store
            .context_brief(query, ContextBriefOptions::default())
            .expect("context brief should succeed");

        assert_eq!(brief, context::NO_RELEVANT_MEMORY);
    }

    #[cfg(unix)]
    #[test]
    fn database_file_is_not_world_writable() {
        use std::os::unix::fs::PermissionsExt;

        let (_store, _tmp, db_path) = test_store();
        let metadata = fs::metadata(db_path).expect("metadata");
        let mode = metadata.permissions().mode();

        assert_eq!(mode & 0o002, 0, "db file must not be world-writable");
    }

    // ----- Conflict detection tests -----------------------------------------

    #[test]
    fn conflicting_decisions_are_detected() {
        let (store, _tmp, _) = test_store();

        let id_a = store
            .insert_memory(NewMemory {
                namespace: Some("project:auth".to_string()),
                memory_type: MemoryType::Decision,
                title: "Use Postgres database for auth storage".to_string(),
                content: "Auth data lives in Postgres".to_string(),
                entities: vec![],
                importance: 70,
                source: None,
            })
            .expect("insert a");

        let id_b = store
            .insert_memory(NewMemory {
                namespace: Some("project:auth".to_string()),
                memory_type: MemoryType::Decision,
                title: "Use MySQL database for auth storage".to_string(),
                content: "Auth data lives in MySQL".to_string(),
                entities: vec![],
                importance: 70,
                source: None,
            })
            .expect("insert b");

        let memory_b = store.get_memory_by_id(&id_b).expect("get b");
        store
            .detect_and_store_conflicts(&memory_b)
            .expect("detect conflicts");

        let conflicts = store.list_conflicts(None).expect("list conflicts");
        assert!(
            !conflicts.is_empty(),
            "expected a conflict between the two database decisions"
        );
        let c = &conflicts[0];
        assert!(
            (c.memory_id_a == id_a && c.memory_id_b == id_b)
                || (c.memory_id_a == id_b && c.memory_id_b == id_a),
            "conflict should reference both inserted memories"
        );
        assert!(
            c.reason.contains("database")
                || c.reason.contains("auth")
                || c.reason.contains("storage"),
            "reason should mention the shared keywords: {}",
            c.reason
        );
    }

    #[test]
    fn non_conflicting_decisions_are_not_detected() {
        let (store, _tmp, _) = test_store();

        store
            .insert_memory(NewMemory {
                namespace: Some("project:auth".to_string()),
                memory_type: MemoryType::Decision,
                title: "Use Postgres for user data".to_string(),
                content: "Users stored in Postgres".to_string(),
                entities: vec![],
                importance: 70,
                source: None,
            })
            .expect("insert a");

        let id_b = store
            .insert_memory(NewMemory {
                namespace: Some("project:auth".to_string()),
                memory_type: MemoryType::Decision,
                title: "Deploy with Docker Compose".to_string(),
                content: "Use Docker Compose for local dev".to_string(),
                entities: vec![],
                importance: 50,
                source: None,
            })
            .expect("insert b");

        let memory_b = store.get_memory_by_id(&id_b).expect("get b");
        store
            .detect_and_store_conflicts(&memory_b)
            .expect("detect conflicts");

        let conflicts = store.list_conflicts(None).expect("list conflicts");
        assert!(
            conflicts.is_empty(),
            "unrelated decisions should not conflict"
        );
    }

    #[test]
    fn conflict_detection_skips_non_eligible_types() {
        let (store, _tmp, _) = test_store();

        // Two tasks with very similar titles — should NOT be flagged.
        store
            .insert_memory(NewMemory {
                namespace: Some("project:auth".to_string()),
                memory_type: MemoryType::Task,
                title: "Update authentication token logic handler".to_string(),
                content: "needs updating".to_string(),
                entities: vec![],
                importance: 50,
                source: None,
            })
            .expect("insert a");

        let id_b = store
            .insert_memory(NewMemory {
                namespace: Some("project:auth".to_string()),
                memory_type: MemoryType::Task,
                title: "Refactor authentication token logic handler".to_string(),
                content: "needs refactoring".to_string(),
                entities: vec![],
                importance: 50,
                source: None,
            })
            .expect("insert b");

        let memory_b = store.get_memory_by_id(&id_b).expect("get b");
        store
            .detect_and_store_conflicts(&memory_b)
            .expect("detect conflicts");

        let conflicts = store.list_conflicts(None).expect("list conflicts");
        assert!(
            conflicts.is_empty(),
            "task type should not trigger conflict detection"
        );
    }

    #[test]
    fn conflicts_do_not_cross_namespace_boundaries() {
        let (store, _tmp, _) = test_store();

        store
            .insert_memory(NewMemory {
                namespace: Some("project:auth".to_string()),
                memory_type: MemoryType::Decision,
                title: "Use Postgres database for auth storage".to_string(),
                content: "auth uses postgres".to_string(),
                entities: vec![],
                importance: 70,
                source: None,
            })
            .expect("insert a");

        let id_b = store
            .insert_memory(NewMemory {
                namespace: Some("project:payments".to_string()),
                memory_type: MemoryType::Decision,
                title: "Use Postgres database for auth storage".to_string(),
                content: "payments uses postgres".to_string(),
                entities: vec![],
                importance: 70,
                source: None,
            })
            .expect("insert b");

        let memory_b = store.get_memory_by_id(&id_b).expect("get b");
        store
            .detect_and_store_conflicts(&memory_b)
            .expect("detect conflicts");

        let conflicts = store.list_conflicts(None).expect("list conflicts");
        assert!(
            conflicts.is_empty(),
            "memories in different namespaces must not conflict"
        );
    }

    #[test]
    fn namespace_filter_on_list_conflicts_works() {
        let (store, _tmp, _) = test_store();

        // Create a conflict in project:auth
        let id_a = store
            .insert_memory(NewMemory {
                namespace: Some("project:auth".to_string()),
                memory_type: MemoryType::Decision,
                title: "Use Postgres database for auth storage".to_string(),
                content: "auth uses postgres".to_string(),
                entities: vec![],
                importance: 70,
                source: None,
            })
            .expect("insert a");
        let id_b = store
            .insert_memory(NewMemory {
                namespace: Some("project:auth".to_string()),
                memory_type: MemoryType::Decision,
                title: "Use MySQL database for auth storage".to_string(),
                content: "auth uses mysql".to_string(),
                entities: vec![],
                importance: 70,
                source: None,
            })
            .expect("insert b");
        let memory_b = store.get_memory_by_id(&id_b).expect("get b");
        store
            .detect_and_store_conflicts(&memory_b)
            .expect("detect conflicts");

        // Filter to project:auth — should see the conflict.
        let auth_conflicts = store
            .list_conflicts(Some("project:auth".to_string()))
            .expect("list auth conflicts");
        assert!(!auth_conflicts.is_empty());

        // Filter to project:payments — should see nothing.
        let payments_conflicts = store
            .list_conflicts(Some("project:payments".to_string()))
            .expect("list payments conflicts");
        assert!(payments_conflicts.is_empty());

        // No filter — same conflict.
        let all_conflicts = store.list_conflicts(None).expect("list all conflicts");
        assert_eq!(all_conflicts.len(), auth_conflicts.len());

        let _ = (id_a, id_b);
    }

    #[test]
    fn conflict_count_returns_unresolved_only() {
        let (store, _tmp, _) = test_store();

        let id_a = store
            .insert_memory(NewMemory {
                namespace: Some("project:auth".to_string()),
                memory_type: MemoryType::Decision,
                title: "Use Postgres database for auth storage".to_string(),
                content: "auth uses postgres".to_string(),
                entities: vec![],
                importance: 70,
                source: None,
            })
            .expect("insert a");
        let id_b = store
            .insert_memory(NewMemory {
                namespace: Some("project:auth".to_string()),
                memory_type: MemoryType::Decision,
                title: "Use MySQL database for auth storage".to_string(),
                content: "auth uses mysql".to_string(),
                entities: vec![],
                importance: 70,
                source: None,
            })
            .expect("insert b");

        let memory_b = store.get_memory_by_id(&id_b).expect("get b");
        store.detect_and_store_conflicts(&memory_b).expect("detect");

        let count = store.conflict_count().expect("count");
        assert_eq!(count, 1);

        let _ = (id_a, id_b);
    }

    #[test]
    fn insert_memory_checked_redacts_secret_and_returns_warning() {
        let (store, _tmp, _) = test_store();
        let memory = NewMemory {
            namespace: Some("project:auth".to_string()),
            memory_type: MemoryType::Fact,
            title: "Deploy token".to_string(),
            content: "the deploy token is ghp_123456789012345678901234567890123456".to_string(),
            entities: vec![],
            importance: 50,
            source: None,
        };

        let (id, warnings) = store
            .insert_memory_checked(memory, false)
            .expect("checked insert should succeed");
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, "secret_redacted");
        assert!(
            !warnings[0].message.contains("ghp_"),
            "warning leaked secret"
        );

        let inserted = store.get_memory_by_id(&id).expect("memory should exist");
        assert_eq!(
            inserted.title, "Deploy token",
            "clean title must be untouched"
        );
        assert!(inserted.content.contains("[REDACTED:github-pat]"));
        assert!(!inserted.content.contains("ghp_"), "secret stored verbatim");
    }

    #[test]
    fn insert_memory_checked_allow_secret_stores_verbatim_without_warnings() {
        let (store, _tmp, _) = test_store();
        let secret = "ghp_123456789012345678901234567890123456";
        let memory = NewMemory {
            namespace: Some("project:auth".to_string()),
            memory_type: MemoryType::Fact,
            title: "Deploy token".to_string(),
            content: format!("the deploy token is {secret}"),
            entities: vec![],
            importance: 50,
            source: None,
        };

        let (id, warnings) = store
            .insert_memory_checked(memory, true)
            .expect("checked insert should succeed");
        assert!(warnings.is_empty());

        let inserted = store.get_memory_by_id(&id).expect("memory should exist");
        assert!(
            inserted.content.contains(secret),
            "allow_secret must keep verbatim"
        );
    }

    #[test]
    fn insert_memory_checked_clean_content_has_no_warnings() {
        let (store, _tmp, _) = test_store();
        let (id, warnings) = store
            .insert_memory_checked(
                sample_new_memory(Some("project:auth"), MemoryType::Decision),
                false,
            )
            .expect("checked insert should succeed");
        assert!(warnings.is_empty());
        let inserted = store.get_memory_by_id(&id).expect("memory should exist");
        assert_eq!(inserted.title, "Use Postgres for auth");
    }

    #[test]
    fn record_access_increments_count_and_stamps_timestamp() {
        let (store, _tmp, _) = test_store();
        let id = store
            .insert_memory(sample_new_memory(Some("project:auth"), MemoryType::Fact))
            .expect("insert");

        let memory = store.get_memory_by_id(&id).expect("get");
        assert_eq!(memory.access_count, 0);
        assert!(memory.last_accessed_at.is_none());

        store
            .record_access(std::slice::from_ref(&id))
            .expect("record");
        store
            .record_access(std::slice::from_ref(&id))
            .expect("record again");

        let memory = store.get_memory_by_id(&id).expect("get");
        assert_eq!(memory.access_count, 2);
        assert!(memory.last_accessed_at.is_some());
    }

    #[test]
    fn record_access_batch_and_empty_are_safe() {
        let (store, _tmp, _) = test_store();
        let id_a = store
            .insert_memory(sample_new_memory(Some("project:auth"), MemoryType::Fact))
            .expect("insert a");
        let id_b = store
            .insert_memory(sample_new_memory(Some("project:auth"), MemoryType::Note))
            .expect("insert b");

        store.record_access(&[]).expect("empty is a no-op");

        store
            .record_access(&[id_a.clone(), id_b.clone()])
            .expect("batch");

        assert_eq!(store.get_memory_by_id(&id_a).unwrap().access_count, 1);
        assert_eq!(store.get_memory_by_id(&id_b).unwrap().access_count, 1);
    }

    #[test]
    fn retrieve_returns_hits_and_memories_in_rank_order() {
        let (store, _tmp, _) = test_store();
        store
            .insert_memory(sample_new_memory(Some("project:auth"), MemoryType::Fact))
            .expect("insert");

        let query = Query::new("postgres".to_string());
        let outcome = store.retrieve(query).expect("retrieve");
        assert_eq!(outcome.hits.len(), 1);
        assert_eq!(outcome.hits[0].id, outcome.memories[0].id);
        assert!(outcome.memories[0].content.contains("Postgres"));
    }
}
