//! Search abstraction and default SQLite FTS5 backend.

use std::collections::HashMap;
use std::path::PathBuf;

use rusqlite::{params, Connection, OpenFlags};
use serde::{Deserialize, Serialize};

use crate::memory::{normalize_namespace, Memory, MemoryType};
use crate::{Error, Result};

/// Search query modeled around Locus caller needs.
#[derive(Debug, Clone)]
pub struct Query {
    pub text: String,
    pub namespace: Option<String>,
    pub memory_type: Option<MemoryType>,
    pub limit: usize,
}

impl Query {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            namespace: None,
            memory_type: None,
            limit: 20,
        }
    }

    pub fn validate(&self) -> Result<()> {
        if self.text.trim().is_empty() {
            return Err(Error::InvalidInput(
                "search query must not be empty".to_string(),
            ));
        }
        if self.limit == 0 {
            return Err(Error::InvalidInput(
                "search limit must be greater than 0".to_string(),
            ));
        }
        Ok(())
    }
}

/// A search candidate produced by an engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hit {
    pub id: String,
    pub relevance: f32,
    pub snippet: String,
}

/// Minimal engine contract used by higher-level layers.
pub trait SearchEngine: Send + Sync {
    fn search(&self, query: &Query) -> Result<Vec<Hit>>;
    fn upsert(&self, memory: &Memory) -> Result<()>;
    fn remove(&self, id: &str) -> Result<()>;
}

/// Default SQLite FTS5 implementation.
#[derive(Debug, Clone)]
pub struct Fts5SearchEngine {
    db_path: PathBuf,
}

impl Fts5SearchEngine {
    pub fn open_at(path: PathBuf) -> Self {
        Self { db_path: path }
    }

    fn connect_ro(&self) -> Result<Connection> {
        let conn = Connection::open_with_flags(
            &self.db_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
        )?;
        conn.execute_batch("PRAGMA busy_timeout = 5000;")?;
        Ok(conn)
    }

    fn connect_rw(&self) -> Result<Connection> {
        let conn = Connection::open(&self.db_path)?;
        conn.execute_batch("PRAGMA busy_timeout = 5000;")?;
        Ok(conn)
    }

    fn phrase_or_term_query(text: &str) -> String {
        let trimmed = text.trim();
        if trimmed.contains('"') || trimmed.contains('*') {
            trimmed.to_string()
        } else {
            // Quote each term so punctuation like `::` is treated as text.
            trimmed
                .split_whitespace()
                .map(|term| format!("\"{}\"", term.replace('"', "")))
                .collect::<Vec<_>>()
                .join(" ")
        }
    }

    fn search_like_fallback(&self, query: &Query) -> Result<Vec<Hit>> {
        let conn = self.connect_ro()?;

        let namespace = query.namespace.as_ref().map(|s| s.trim().to_string());
        let memory_type = query.memory_type.map(MemoryType::as_str);

        let mut sql = String::from(
            "
            SELECT
                m.id,
                substr(m.title || ' - ' || m.content, 1, 180) AS snippet
            FROM memories m
            WHERE lower(
                m.title || ' ' || m.content || ' ' || COALESCE((
                    SELECT group_concat(e.name, ' ')
                    FROM memory_entities me
                    INNER JOIN entities e ON e.id = me.entity_id
                    WHERE me.memory_id = m.id
                ), '')
            ) LIKE '%' || lower(?) || '%'
            ",
        );

        let mut dynamic_params: Vec<String> = vec![query.text.trim().to_string()];

        if let Some(ns) = namespace {
            sql.push_str(" AND m.namespace = ?");
            dynamic_params.push(normalize_namespace(Some(ns)));
        }

        if let Some(kind) = memory_type {
            sql.push_str(" AND m.type = ?");
            dynamic_params.push(kind.to_string());
        }

        sql.push_str(" ORDER BY m.updated_at DESC LIMIT ?");
        let limit_i64 = i64::try_from(query.limit)
            .map_err(|_| Error::InvalidInput("search limit is too large".to_string()))?;

        let mut stmt = conn.prepare(&sql)?;
        let mut values = dynamic_params
            .iter()
            .map(|value| value as &dyn rusqlite::ToSql)
            .collect::<Vec<_>>();
        values.push(&limit_i64);

        let rows = stmt
            .query_map(values.as_slice(), |row| {
                Ok(Hit {
                    id: row.get(0)?,
                    relevance: 0.01,
                    snippet: row.get(1)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(rows)
    }
}

impl SearchEngine for Fts5SearchEngine {
    fn search(&self, query: &Query) -> Result<Vec<Hit>> {
        query.validate()?;
        let conn = self.connect_ro()?;

        let fts_query = Self::phrase_or_term_query(&query.text);

        let mut sql = String::from(
            "
            SELECT
                m.id,
                bm25(memory_fts) AS rank_score,
                snippet(memory_fts, 2, '[', ']', '...', 10) AS snippet
            FROM memory_fts
            INNER JOIN memories m ON m.id = memory_fts.memory_id
            WHERE memory_fts MATCH ?
            ",
        );

        let mut dynamic_params: Vec<String> = vec![fts_query];

        if let Some(namespace) = &query.namespace {
            sql.push_str(" AND m.namespace = ?");
            dynamic_params.push(normalize_namespace(Some(namespace.clone())));
        }

        if let Some(memory_type) = query.memory_type {
            sql.push_str(" AND m.type = ?");
            dynamic_params.push(memory_type.as_str().to_string());
        }

        sql.push_str(" ORDER BY rank_score ASC LIMIT ?");
        let limit_i64 = i64::try_from(query.limit)
            .map_err(|_| Error::InvalidInput("search limit is too large".to_string()))?;

        let mut stmt = conn.prepare(&sql)?;
        let mut values = dynamic_params
            .iter()
            .map(|value| value as &dyn rusqlite::ToSql)
            .collect::<Vec<_>>();
        values.push(&limit_i64);

        let mut rows = stmt.query(values.as_slice())?;
        let mut hits = Vec::new();

        while let Some(row) = rows.next()? {
            let raw_rank: f64 = row.get(1)?;
            let snippet: Option<String> = row.get(2)?;

            hits.push(Hit {
                id: row.get(0)?,
                // FTS5 bm25 score is lower-is-better; invert to higher-is-better.
                relevance: (-raw_rank) as f32,
                snippet: snippet.unwrap_or_default(),
            });
        }

        if hits.is_empty() {
            return self.search_like_fallback(query);
        }

        Ok(hits)
    }

    fn upsert(&self, memory: &Memory) -> Result<()> {
        let conn = self.connect_rw()?;
        conn.execute(
            "DELETE FROM memory_fts WHERE memory_id = ?",
            params![memory.id],
        )?;

        conn.execute(
            "
            INSERT INTO memory_fts (memory_id, title, content, entities)
            VALUES (?, ?, ?, ?)
            ",
            params![
                memory.id,
                memory.title,
                memory.content,
                memory.entities.join(" ")
            ],
        )?;
        Ok(())
    }

    fn remove(&self, id: &str) -> Result<()> {
        if id.trim().is_empty() {
            return Err(Error::InvalidInput("id must not be empty".to_string()));
        }

        let conn = self.connect_rw()?;
        conn.execute("DELETE FROM memory_fts WHERE memory_id = ?", params![id])?;
        Ok(())
    }
}

/// Extra ranking signals applied above engine-level lexical relevance.
#[derive(Debug, Clone, Copy)]
pub struct RankSignals {
    pub importance: u8,
    pub updated_at: i64,
}

/// Shared, engine-agnostic reranker.
pub fn rerank_hits(mut hits: Vec<Hit>, signals: &HashMap<String, RankSignals>) -> Vec<Hit> {
    if hits.is_empty() {
        return hits;
    }

    let timestamps = signals
        .values()
        .map(|signal| signal.updated_at)
        .collect::<Vec<_>>();
    let min_ts = timestamps.iter().copied().min().unwrap_or(0);
    let max_ts = timestamps.iter().copied().max().unwrap_or(0);

    hits.sort_by(|left, right| {
        let left_score = composite_score(left, signals.get(&left.id), min_ts, max_ts);
        let right_score = composite_score(right, signals.get(&right.id), min_ts, max_ts);

        right_score
            .partial_cmp(&left_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    hits
}

fn composite_score(hit: &Hit, signal: Option<&RankSignals>, min_ts: i64, max_ts: i64) -> f32 {
    let (importance, updated_at) = match signal {
        Some(value) => (f32::from(value.importance), value.updated_at),
        None => (0.0, min_ts),
    };

    let normalized_importance = importance / 100.0;
    let normalized_recency = normalize_i64(updated_at, min_ts, max_ts);

    // Engine relevance stays primary; boosts reorder close-scoring candidates.
    hit.relevance + (normalized_recency * 0.05) + (normalized_importance * 0.03)
}

fn normalize_i64(value: i64, min: i64, max: i64) -> f32 {
    if max == min {
        return 1.0;
    }
    (value - min) as f32 / (max - min) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reranker_prefers_newer_and_more_important_when_relevance_is_close() {
        let hits = vec![
            Hit {
                id: "older".to_string(),
                relevance: 0.950,
                snippet: String::new(),
            },
            Hit {
                id: "newer".to_string(),
                relevance: 0.949,
                snippet: String::new(),
            },
        ];

        let mut signals = HashMap::new();
        signals.insert(
            "older".to_string(),
            RankSignals {
                importance: 20,
                updated_at: 1_000,
            },
        );
        signals.insert(
            "newer".to_string(),
            RankSignals {
                importance: 95,
                updated_at: 2_000,
            },
        );

        let ranked = rerank_hits(hits, &signals);
        assert_eq!(ranked[0].id, "newer");
    }
}
