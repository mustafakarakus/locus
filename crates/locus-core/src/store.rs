//! SQLite-backed canonical memory store.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use time::OffsetDateTime;
use uuid::Uuid;

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
];

/// SQLite-backed storage for canonical memories.
#[derive(Debug, Clone)]
pub struct Store {
    db_path: PathBuf,
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
        let affected = tx.execute("DELETE FROM memories WHERE id = ?", params![id])?;
        if affected == 0 {
            return Err(Error::NotFound("memory not found".to_string()));
        }

        tx.execute("DELETE FROM memory_fts WHERE memory_id = ?", params![id])?;
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

    /// Fetches a memory by id.
    pub fn get_memory_by_id(&self, id: &str) -> Result<Memory> {
        if id.trim().is_empty() {
            return Err(Error::InvalidInput("id must not be empty".to_string()));
        }

        let conn = self.connect_ro()?;
        let row = conn
            .query_row(
                "
                SELECT id, namespace, type, title, content, importance, source, created_at, updated_at
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
            SELECT id, namespace, type, title, content, importance, source, created_at, updated_at
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

    fn connect_rw(&self) -> Result<Connection> {
        let conn = Connection::open(&self.db_path)?;
        apply_pragmas(&conn)?;
        Ok(conn)
    }

    fn connect_ro(&self) -> Result<Connection> {
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

    tx.execute(
        "DELETE FROM memory_fts WHERE memory_id = ?",
        params![memory_id],
    )?;
    tx.execute(
        "
        INSERT INTO memory_fts (memory_id, title, content, entities)
        VALUES (?, ?, ?, ?)
        ",
        params![memory_id, title, content, entities],
    )?;
    Ok(())
}

fn load_entities(conn: &Connection, memory_id: &str) -> Result<Vec<String>> {
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

    #[cfg(unix)]
    #[test]
    fn database_file_is_not_world_writable() {
        use std::os::unix::fs::PermissionsExt;

        let (_store, _tmp, db_path) = test_store();
        let metadata = fs::metadata(db_path).expect("metadata");
        let mode = metadata.permissions().mode();

        assert_eq!(mode & 0o002, 0, "db file must not be world-writable");
    }
}
