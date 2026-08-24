//! Search abstraction and default SQLite FTS5 backend.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};

use crate::memory::{normalize_namespace, Memory, MemoryType};
use crate::{Error, Result};

/// Shortest term kept as a query token. One-character tokens are noise in FTS5
/// (prefix indexes start at 2) and would make LIKE `%a%` match almost everything.
const MIN_TERM_CHARS: usize = 2;
/// Function and question words agents paste into MCP queries. They are not
/// content and would OR-match almost every memory (`the`, `what`, `was`).
const SEARCH_STOP_WORDS: &[&str] = &[
    "a", "about", "all", "also", "am", "an", "and", "any", "are", "at", "be", "been", "being",
    "but", "by", "can", "could", "describe", "did", "do", "does", "explain", "find", "for", "from",
    "get", "give", "had", "has", "have", "how", "i", "if", "in", "into", "is", "it", "its", "just",
    "look", "may", "me", "might", "my", "no", "not", "of", "on", "only", "onto", "or", "our",
    "over", "please", "really", "shall", "should", "show", "so", "tell", "than", "that", "the",
    "their", "them", "then", "these", "they", "this", "those", "to", "use", "used", "using",
    "very", "was", "we", "were", "what", "when", "where", "which", "who", "whom", "whose", "why",
    "will", "with", "without", "would", "yes", "you", "your",
];
/// Fetch this many extra FTS candidates before coverage-filtering down to `limit`.
const CANDIDATE_MULTIPLIER: usize = 5;
const CANDIDATE_FLOOR: usize = 24;

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
    /// Fraction of query terms present in the memory (0.0–1.0).
    ///
    /// Filled by the shared coverage layer above the engine, not by FTS5 itself.
    #[serde(default)]
    pub coverage: f32,
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

    fn compile_fts_match(text: &str) -> String {
        let trimmed = text.trim();
        // Prefix operators and unbalanced quotes are FTS5 syntax; pass through
        // so phrase/prefix still work and syntax errors can fall back to LIKE.
        if trimmed.contains('*') || has_unbalanced_quotes(trimmed) {
            return trimmed.to_string();
        }

        let terms = extract_query_terms(trimmed);
        if terms.is_empty() {
            return trimmed
                .split_whitespace()
                .map(|term| format!("\"{}\"", term.replace('"', "")))
                .collect::<Vec<_>>()
                .join(" OR ");
        }

        terms
            .iter()
            .map(|term| format!("\"{}\"", term.replace('"', "")))
            .collect::<Vec<_>>()
            .join(" OR ")
    }

    pub(crate) fn search_like_fallback(&self, query: &Query) -> Result<Vec<Hit>> {
        let conn = self.connect_ro()?;

        let namespace = query.namespace.as_ref().map(|s| s.trim().to_string());
        let memory_type = query.memory_type.map(MemoryType::as_str);

        let terms = extract_query_terms(&query.text);
        let patterns: Vec<String> = if terms.is_empty() {
            vec![escape_like(&query.text.trim().to_lowercase())]
        } else {
            terms.iter().map(|term| escape_like(term)).collect()
        };

        // Scan the FTS shadow table directly: it already carries title, content
        // and the concatenated entity names for every memory, so a LIKE filter
        // needs no per-row correlated subquery into memory_entities/entities.
        let mut sql = String::from(
            "
            SELECT
                f.memory_id,
                substr(f.title || ' - ' || f.content, 1, 180) AS snippet
            FROM memory_fts f
            INNER JOIN memories m ON m.id = f.memory_id
            WHERE (
            ",
        );

        for index in 0..patterns.len() {
            if index > 0 {
                sql.push_str(" OR ");
            }
            sql.push_str(
                "lower(f.title || ' ' || f.content || ' ' || f.entities) LIKE '%' || ? || '%' ESCAPE '\\'",
            );
        }
        sql.push(')');

        let mut dynamic_params: Vec<String> = patterns;

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
                    coverage: 0.0,
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

        let fts_query = Self::compile_fts_match(&query.text);

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

        // FTS5 raises a syntax error (e.g. an unbalanced quote or a bare `*`)
        // for queries that were passed through verbatim. That must not fail the
        // whole search — degrade to the LIKE scan instead. This is safe: if the
        // underlying database were genuinely broken, the LIKE scan would fail
        // with its own error rather than silently returning results.
        let fts = (|| -> Result<Vec<Hit>> {
            let mut rows = stmt.query(values.as_slice())?;
            let mut hits = Vec::new();

            while let Some(row) = rows.next()? {
                let raw_rank: f64 = row.get(1)?;
                let snippet: Option<String> = row.get(2)?;

                hits.push(Hit {
                    id: row.get(0)?,
                    // FTS5 bm25 score is lower-is-better; invert to higher-is-better.
                    relevance: (-raw_rank) as f32,
                    coverage: 0.0,
                    snippet: snippet.unwrap_or_default(),
                });
            }

            Ok(hits)
        })();

        let hits = match fts {
            Ok(hits) if !hits.is_empty() => hits,
            Ok(_) => return self.search_like_fallback(query),
            Err(crate::Error::Sql(rusqlite::Error::SqliteFailure(_, _))) => {
                return self.search_like_fallback(query);
            }
            Err(err) => return Err(err),
        };

        Ok(hits)
    }

    fn upsert(&self, memory: &Memory) -> Result<()> {
        let mut conn = self.connect_rw()?;
        let tx = conn.transaction()?;

        let existing_rowid: Option<i64> = tx
            .query_row(
                "SELECT fts_rowid FROM memory_fts_rowid WHERE memory_id = ?",
                params![memory.id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(rowid) = existing_rowid {
            tx.execute("DELETE FROM memory_fts WHERE rowid = ?", params![rowid])?;
            tx.execute(
                "DELETE FROM memory_fts_rowid WHERE memory_id = ?",
                params![memory.id],
            )?;
        }

        tx.execute(
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
        let new_rowid = tx.last_insert_rowid();
        tx.execute(
            "INSERT INTO memory_fts_rowid (memory_id, fts_rowid) VALUES (?, ?)",
            params![memory.id, new_rowid],
        )?;
        tx.commit()?;
        Ok(())
    }

    fn remove(&self, id: &str) -> Result<()> {
        if id.trim().is_empty() {
            return Err(Error::InvalidInput("id must not be empty".to_string()));
        }

        let mut conn = self.connect_rw()?;
        let tx = conn.transaction()?;

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
        tx.execute(
            "DELETE FROM memory_fts_rowid WHERE memory_id = ?",
            params![id],
        )?;
        tx.commit()?;
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
    hit.relevance
        + (normalized_recency * 0.05)
        + (normalized_importance * 0.03)
        + (hit.coverage * 0.1)
}

fn has_unbalanced_quotes(text: &str) -> bool {
    text.chars().filter(|ch| *ch == '"').count() % 2 == 1
}

fn strip_wrapping_punct(value: &str) -> &str {
    value.trim_matches(|c: char| !(c.is_alphanumeric() || matches!(c, '-' | '_' | ':' | '.' | '/')))
}

fn push_term(terms: &mut Vec<String>, current: &mut String) {
    let raw = std::mem::take(current);
    let stripped = strip_wrapping_punct(raw.trim().trim_matches('*').trim());
    if stripped.chars().count() < MIN_TERM_CHARS {
        return;
    }
    let lower = stripped.to_lowercase();
    if SEARCH_STOP_WORDS.contains(&lower.as_str()) {
        return;
    }
    terms.push(lower);
}

/// Split a query into terms for OR retrieval and coverage scoring.
///
/// Agents send questions over MCP (`what was the local api decision?`); this
/// keeps content tokens only. Quoted spans stay a single term
/// (`"token verification"`). `*` and wrapping punctuation are stripped.
/// One-character tokens and stopwords are dropped. Duplicates are removed,
/// first occurrence kept.
pub(crate) fn extract_query_terms(text: &str) -> Vec<String> {
    let mut terms = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;

    for ch in text.chars() {
        match ch {
            '"' => {
                if in_quotes {
                    push_term(&mut terms, &mut current);
                    in_quotes = false;
                } else {
                    push_term(&mut terms, &mut current);
                    in_quotes = true;
                }
            }
            c if c.is_whitespace() && !in_quotes => push_term(&mut terms, &mut current),
            _ => current.push(ch),
        }
    }
    push_term(&mut terms, &mut current);

    if terms.is_empty() {
        let fallback = text
            .chars()
            .filter(|ch| *ch != '"' && *ch != '*')
            .collect::<String>()
            .trim()
            .to_lowercase();
        if !fallback.is_empty() {
            terms.push(fallback);
        }
    }

    let mut seen = HashSet::new();
    terms.retain(|term| seen.insert(term.clone()));
    terms
}

fn escape_like(term: &str) -> String {
    let mut escaped = String::with_capacity(term.len());
    for ch in term.chars() {
        match ch {
            '\\' | '%' | '_' => {
                escaped.push('\\');
                escaped.push(ch);
            }
            _ => escaped.push(ch),
        }
    }
    escaped
}

/// Minimum number of query terms a memory must contain to be considered relevant.
///
/// One-term queries stay exact. Two-term queries stay AND-like (both required)
/// so `"token verification"`-style pairs do not collapse to either word.
/// Longer agent queries require at least two terms, or 30% of terms, whichever
/// is larger — enough to drop a single generic token like `POSTs` without
/// dropping a memory that matches most of a seven-term paste.
pub(crate) fn min_term_matches(term_count: usize) -> usize {
    match term_count {
        0 => 1,
        1 => 1,
        2 => 2,
        3 | 4 => 2,
        n => (n * 3 / 10).max(2),
    }
}

pub(crate) fn term_coverage(haystack_lower: &str, terms: &[String]) -> (usize, f32) {
    if terms.is_empty() {
        return (0, 0.0);
    }
    let matched = terms
        .iter()
        .filter(|term| haystack_lower.contains(term.as_str()))
        .count();
    (matched, matched as f32 / terms.len() as f32)
}

pub fn coverage_percent(coverage: f32) -> u8 {
    (coverage * 100.0).round().clamp(0.0, 100.0) as u8
}

pub(crate) fn candidate_limit(requested: usize) -> usize {
    requested
        .saturating_mul(CANDIDATE_MULTIPLIER)
        .max(CANDIDATE_FLOOR)
}

pub(crate) fn memory_haystack(
    title: &str,
    content: &str,
    entities: &[String],
    memory_type: MemoryType,
) -> String {
    // Type lives in `memories.type`, not FTS text. Include it so queries like
    // "tts decision" can match a decision that never says the word "decision".
    format!(
        "{} {} {} {}",
        title,
        content,
        entities.join(" "),
        memory_type.as_str()
    )
    .to_lowercase()
}

pub(crate) fn filter_hits_by_coverage(
    hits: Vec<Hit>,
    haystacks: &HashMap<String, String>,
    terms: &[String],
) -> Vec<Hit> {
    if terms.is_empty() {
        return hits;
    }
    let min_matches = min_term_matches(terms.len());
    hits.into_iter()
        .filter_map(|mut hit| {
            let haystack = haystacks.get(&hit.id).map(String::as_str).unwrap_or("");
            let (matched, coverage) = term_coverage(haystack, terms);
            if matched < min_matches {
                return None;
            }
            hit.coverage = coverage;
            Some(hit)
        })
        .collect()
}

fn normalize_i64(value: i64, min: i64, max: i64) -> f32 {
    if max == min {
        return 1.0;
    }
    (value - min) as f32 / (max - min) as f32
}

#[cfg(test)]
mod tests {
    use crate::store::Store;
    use tempfile::TempDir;

    use super::*;

    fn test_store() -> (Store, TempDir) {
        let tmp = TempDir::new().expect("temp dir");
        let store = Store::open_at(tmp.path().join("locus.db")).expect("store should initialize");
        (store, tmp)
    }

    #[test]
    fn fts_syntax_error_falls_back_to_like() {
        let (store, _tmp) = test_store();
        store
            .insert_memory(crate::memory::NewMemory {
                namespace: Some("global".to_string()),
                memory_type: MemoryType::Fact,
                title: "Quoting rules".to_string(),
                content: "Always quote the tokens he said".to_string(),
                entities: vec![],
                importance: 50,
                source: None,
            })
            .expect("insert");

        // An unbalanced quote is a fatal FTS5 syntax error. It must not hard-
        // fail the search: degrade to the LIKE scan and still return the hit.
        let engine = Fts5SearchEngine::open_at(store.db_path().to_path_buf());
        let hits = engine
            .search(&Query::new("\"unbalanced"))
            .expect("syntax-error query must not fail");
        assert!(hits.is_empty(), "no text matches the malformed phrase");

        let hits = engine
            .search(&Query::new("he said"))
            .expect("valid query must succeed");
        assert!(
            hits.iter().any(|h| h.snippet.contains("quote the tokens")),
            "LIKE fallback must find the memory"
        );
    }

    #[test]
    fn reranker_prefers_newer_and_more_important_when_relevance_is_close() {
        let hits = vec![
            Hit {
                id: "older".to_string(),
                relevance: 0.950,
                coverage: 0.0,
                snippet: String::new(),
            },
            Hit {
                id: "newer".to_string(),
                relevance: 0.949,
                coverage: 0.0,
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

    #[test]
    fn unquoted_terms_compile_to_fts_or() {
        let compiled = Fts5SearchEngine::compile_fts_match(
            "TtsClient POSTs audio speech X-Language X-Engine chatterbox",
        );
        assert_eq!(
            compiled,
            "\"ttsclient\" OR \"posts\" OR \"audio\" OR \"speech\" OR \"x-language\" OR \"x-engine\" OR \"chatterbox\""
        );
    }

    #[test]
    fn quoted_phrase_stays_one_term() {
        let compiled = Fts5SearchEngine::compile_fts_match("\"token verification\"");
        assert_eq!(compiled, "\"token verification\"");
        assert_eq!(
            extract_query_terms("hello \"token verification\" world"),
            vec![
                "hello".to_string(),
                "token verification".to_string(),
                "world".to_string()
            ]
        );
    }

    #[test]
    fn question_query_keeps_content_terms_only() {
        assert_eq!(
            extract_query_terms("what was the local api decision?"),
            vec![
                "local".to_string(),
                "api".to_string(),
                "decision".to_string()
            ]
        );
        assert_eq!(
            Fts5SearchEngine::compile_fts_match("what was the local api decision?"),
            "\"local\" OR \"api\" OR \"decision\""
        );
    }

    #[test]
    fn prefix_query_is_passed_through() {
        assert_eq!(Fts5SearchEngine::compile_fts_match("post*"), "post*");
    }

    #[test]
    fn two_term_queries_require_both_terms() {
        assert_eq!(min_term_matches(1), 1);
        assert_eq!(min_term_matches(2), 2);
        assert_eq!(min_term_matches(3), 2);
        assert_eq!(min_term_matches(7), 2);
        assert_eq!(min_term_matches(10), 3);
    }

    #[test]
    fn coverage_filter_drops_single_term_overlap_on_long_queries() {
        let terms =
            extract_query_terms("TtsClient POSTs audio speech X-Language X-Engine chatterbox");
        assert_eq!(terms.len(), 7);

        let hits = vec![
            Hit {
                id: "full".to_string(),
                relevance: 10.0,
                coverage: 0.0,
                snippet: String::new(),
            },
            Hit {
                id: "posts-only".to_string(),
                relevance: 1.0,
                coverage: 0.0,
                snippet: String::new(),
            },
            Hit {
                id: "chatterbox-only".to_string(),
                relevance: 1.0,
                coverage: 0.0,
                snippet: String::new(),
            },
        ];
        let mut haystacks = HashMap::new();
        haystacks.insert(
            "full".to_string(),
            memory_haystack(
                "UC-119 TTS",
                "TtsClient POSTs audio/speech X-Language X-Engine primary chatterbox",
                &[],
                MemoryType::Decision,
            ),
        );
        haystacks.insert(
            "posts-only".to_string(),
            memory_haystack(
                "LLM",
                "LlmClient POSTs chat completions",
                &[],
                MemoryType::Code,
            ),
        );
        haystacks.insert(
            "chatterbox-only".to_string(),
            memory_haystack("Speed", "Chatterbox CUDA speedup", &[], MemoryType::Note),
        );

        let kept = filter_hits_by_coverage(hits, &haystacks, &terms);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].id, "full");
        assert_eq!(coverage_percent(kept[0].coverage), 100);
    }

    #[test]
    fn type_name_in_query_matches_memory_type() {
        let terms = extract_query_terms("tts decision");
        assert_eq!(terms, vec!["tts".to_string(), "decision".to_string()]);

        let hits = vec![
            Hit {
                id: "typed".to_string(),
                relevance: 10.0,
                coverage: 0.0,
                snippet: String::new(),
            },
            Hit {
                id: "note".to_string(),
                relevance: 8.0,
                coverage: 0.0,
                snippet: String::new(),
            },
        ];
        let mut haystacks = HashMap::new();
        haystacks.insert(
            "typed".to_string(),
            memory_haystack(
                "UC-119 TTS",
                "TtsClient POSTs audio/speech",
                &[],
                MemoryType::Decision,
            ),
        );
        haystacks.insert(
            "note".to_string(),
            memory_haystack("TTS speed", "Chatterbox CUDA notes", &[], MemoryType::Note),
        );

        let kept = filter_hits_by_coverage(hits, &haystacks, &terms);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].id, "typed");
        assert_eq!(coverage_percent(kept[0].coverage), 100);
    }

    #[test]
    fn like_fallback_matches_any_term_not_the_whole_sentence() {
        let (store, _tmp) = test_store();
        store
            .insert_memory(crate::memory::NewMemory {
                namespace: Some("global".to_string()),
                memory_type: MemoryType::Fact,
                title: "Verifier function".to_string(),
                content: "Call verify_token_handler from auth API route".to_string(),
                entities: vec!["verify_token_handler".to_string()],
                importance: 40,
                source: None,
            })
            .expect("insert");

        let engine = Fts5SearchEngine::open_at(store.db_path().to_path_buf());
        let hits = engine
            .search_like_fallback(&Query::new("fy_token_han missingterm"))
            .expect("LIKE fallback must succeed");
        assert!(
            hits.iter()
                .any(|h| h.snippet.contains("verify_token_handler")),
            "per-term LIKE must match a substring of one term, not the whole query"
        );
    }
}
