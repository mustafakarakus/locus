//! Session compaction capture (U-017).
//!
//! When a host (Cursor, Claude Code, Copilot, DeepSeek) compacts a session
//! context at ~90-100% full, the resulting summary is the highest-signal record
//! of what the session produced — decisions, preferences, constraints — yet it
//! is normally thrown away. This module captures it: a deterministic,
//! rule-based extractor (no LLM call) splits the compacted text into discrete,
//! typed memories and writes them through the existing store path.
//!
//! This is the write-side mirror of U-015 (hook-based injection). U-015 injects
//! a brief before reasoning; U-017 captures when context is reset. Each host
//! adapter maps its compaction lifecycle event to the single internal
//! [`capture`] call.
//!
//! Design constraints (U-017):
//! - extraction is deterministic — the same text always yields the same
//!   memories, regardless of host or model
//! - extraction is zero-cost (local, microseconds)
//! - captures write discrete typed memories through the existing store write
//!   path (namespace, redaction, dedupe) — never a raw summary blob
//! - no LLM dependency in the default path
//!
//! # Extractor heuristics
//!
//! 1. Split the compacted summary into sentences on `.`, `!`, `?`, `;`, and
//!    newlines ([`split_sentences`]). Bullet/number markers are stripped.
//! 2. Classify each sentence into a [`MemoryType`] by cue-word strength
//!    (highest-strength cue wins), in this order: **Constraint** ("must not",
//!    "cannot", "requires", "prohibited", ...) → **Decision** ("use", "choose",
//!    "decided", "standardize", "migrate to", ...) → **Preference** ("prefer",
//!    "avoid", "always", "never", "favor", ...) → **Task** ("in progress",
//!    "next step", "todo", "pending", "wip", ...). Sentences with no cue fall
//!    back to `Fact`.
//! 3. Title = the leading phrase of the sentence; content = the full sentence.
//! 4. Importance by cue strength: Constraint 75, Decision 70, Preference 65,
//!    Fact/Note 50, Task 40.
//! 5. Entities come from [`extract_entities`] (proper-noun/camelCase tokens)
//!    run through the existing [`crate::memory::normalize_entities`].
//! 6. Candidates are deduped against the existing memories in the target
//!    namespace using `normalize_for_dedupe` + `near_duplicate`.
//!
//! Only durable categories (Constraint/Decision/Preference/Fact) are written;
//! Task-state sentences are skipped as session-transient.

use std::collections::HashSet;

use crate::context::{near_duplicate, normalize_for_dedupe};
use crate::hooks::project_namespace;
use crate::memory::{normalize_entities, ListFilter, MemoryType, NewMemory};
use crate::store::Store;
use crate::{Error, Result};

/// A single memory extracted from a compacted session summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedMemory {
    pub memory_type: MemoryType,
    pub title: String,
    pub content: String,
    pub importance: u8,
    pub entities: Vec<String>,
}

/// Normalized, host-agnostic request to capture a compaction.
///
/// `text` is the compacted summary; `namespace` scopes the captured memories.
/// Every adapter maps its host payload onto exactly this shape.
#[derive(Debug, Clone)]
pub struct CaptureTrigger {
    /// Resolved namespace (e.g. `project:my-app`), derived from host payload.
    pub namespace: Option<String>,
    /// The compacted session summary to extract memories from.
    pub text: String,
}

/// Outcome of a capture: how many memories were written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureOutcome {
    /// Number of memories written (after dedupe and redaction).
    pub written: usize,
    /// Number of extracted sentences skipped as session-transient task state.
    pub skipped_tasks: usize,
    /// Number of extracted sentences that duplicated an existing memory.
    pub skipped_duplicates: usize,
    /// Ids of the memories written by this capture, so the daemon can emit
    /// live events (U-016) for exactly what was captured.
    pub written_ids: Vec<String>,
}

/// Contract every capture adapter implements.
///
/// Adapters must be total: on an unparseable payload they return an
/// [`Error`], which callers treat as graceful degradation (never block the
/// host). They only translate the payload; the [`capture`] function does the
/// extraction and writing.
pub trait CaptureAdapter: Send + Sync {
    /// Stable host identifier (e.g. `claude-code`).
    fn host(&self) -> &'static str;

    /// Translate a host compaction payload into a [`CaptureTrigger`].
    fn translate(&self, payload: &str) -> Result<CaptureTrigger>;
}

/// Resolve a capture adapter by host name. New hosts are separate, explicit
/// work (mirrors U-015).
pub fn adapter_for(host: &str) -> Result<Box<dyn CaptureAdapter>> {
    match host.trim() {
        "claude-code" => Ok(Box::new(ClaudeCodeAdapter)),
        other => Err(Error::InvalidInput(format!(
            "unknown capture host: {other}"
        ))),
    }
}

/// Adapter for Claude Code session compaction.
///
/// Reads the JSON payload passed on stdin and extracts the compacted summary
/// from `summary` (falling back to `compacted_text`) and the namespace from
/// `cwd`.
pub struct ClaudeCodeAdapter;

impl CaptureAdapter for ClaudeCodeAdapter {
    fn host(&self) -> &'static str {
        "claude-code"
    }

    fn translate(&self, payload: &str) -> Result<CaptureTrigger> {
        let value: serde_json::Value = serde_json::from_str(payload).map_err(|err| {
            Error::InvalidInput(format!(
                "claude-code capture payload is not valid JSON: {err}"
            ))
        })?;

        let namespace = value
            .get("cwd")
            .and_then(|v| v.as_str())
            .and_then(project_namespace);

        let text = value
            .get("summary")
            .or_else(|| value.get("compacted_text"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                Error::InvalidInput(
                    "claude-code capture payload missing summary/compacted_text".to_string(),
                )
            })?;

        Ok(CaptureTrigger {
            namespace,
            text: text.to_string(),
        })
    }
}

/// Shared capture path: extract from the compacted summary and write discrete
/// typed memories through the existing store path.
///
/// Scoped to the trigger's namespace (defaults to `global`), deduped against
/// existing memories in that namespace, and passed through U-011 redaction on
/// every write. Task-state sentences (session-transient) are skipped.
pub fn capture(store: &Store, trigger: &CaptureTrigger) -> Result<CaptureOutcome> {
    let namespace = trigger
        .namespace
        .clone()
        .unwrap_or_else(|| "global".to_string());

    let existing = store.list_memories(ListFilter {
        namespace: Some(namespace.clone()),
        ..ListFilter::default()
    })?;
    let mut existing_norms: Vec<String> = existing
        .iter()
        .map(|m| normalize_for_dedupe(&format!("{} {}", m.title, m.content)))
        .collect();

    let mut outcome = CaptureOutcome {
        written: 0,
        skipped_tasks: 0,
        skipped_duplicates: 0,
        written_ids: Vec::new(),
    };

    for candidate in extract_memories(&trigger.text) {
        if candidate.memory_type == MemoryType::Task {
            outcome.skipped_tasks += 1;
            continue;
        }

        let norm = normalize_for_dedupe(&format!("{} {}", candidate.title, candidate.content));
        let is_duplicate = existing_norms
            .iter()
            .any(|existing| existing == &norm || near_duplicate(existing, &norm));
        if is_duplicate {
            outcome.skipped_duplicates += 1;
            continue;
        }

        let input = NewMemory {
            namespace: Some(namespace.clone()),
            memory_type: candidate.memory_type,
            title: candidate.title,
            content: candidate.content,
            entities: candidate.entities,
            importance: candidate.importance,
            source: Some("session:compaction".to_string()),
        };
        let (id, _) = store.insert_memory_checked(input, false)?;
        existing_norms.push(norm);
        outcome.written += 1;
        outcome.written_ids.push(id);
    }

    Ok(outcome)
}

/// Splits compacted summary text into sentences.
///
/// Splits on `.`/`!`/`?`/`;`/newlines, strips bullet markers, collapses
/// whitespace, and drops empty or near-empty fragments. Deterministic.
pub fn split_sentences(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();

    for token in text.split_inclusive(['.', '!', '?', '\n', ';']) {
        current.push_str(token);
        if token.ends_with(['.', '!', '?', '\n', ';']) {
            let clean = clean_sentence(&current);
            if !clean.is_empty() {
                out.push(clean);
            }
            current.clear();
        }
    }
    let tail = clean_sentence(&current);
    if !tail.is_empty() {
        out.push(tail);
    }

    out
}

/// Trims whitespace and bullet/number markers, collapses runs of whitespace.
fn clean_sentence(raw: &str) -> String {
    let trimmed = raw
        .trim()
        .trim_start_matches(['-', '*', '•'])
        .trim()
        .trim_start_matches(|c: char| c.is_ascii_digit())
        .trim_start_matches(['.', ')'])
        .trim();

    let words: Vec<&str> = trimmed.split_whitespace().collect();
    if words.len() < 3 {
        return String::new();
    }
    let joined = words.join(" ");
    if joined.chars().count() < 20 {
        return String::new();
    }
    joined
}

/// Cue words that indicate a durable memory category.
const DECISION_CUES: &[&str] = &[
    "use ",
    "choose ",
    "standardize",
    "decided",
    "we'll use",
    "will use",
    "switch to",
    "adopt",
    "migrate to",
];
const PREFERENCE_CUES: &[&str] = &["prefer", "avoid", "always ", "never ", "like to", "favor"];
const CONSTRAINT_CUES: &[&str] = &[
    "must not",
    "mustn't",
    "cannot",
    "can't",
    "requires",
    "required",
    "is forbidden",
    "prohibited",
    "must",
    "has to",
];
const TASK_CUES: &[&str] = &[
    "in progress",
    "next step",
    "todo",
    "to do",
    "pending",
    "still need",
    "wip",
    "not yet done",
];

/// Classifies a sentence into a [`MemoryType`] via cue words.
///
/// Constraint cues take priority (they are the strongest signal), then
/// decisions, then preferences, then tasks. An unrelated sentence falls back to
/// `Fact`.
pub fn classify(sentence: &str) -> MemoryType {
    let lower = sentence.to_lowercase();
    if CONSTRAINT_CUES.iter().any(|cue| lower.contains(cue)) {
        MemoryType::Constraint
    } else if DECISION_CUES.iter().any(|cue| lower.contains(cue)) {
        MemoryType::Decision
    } else if PREFERENCE_CUES.iter().any(|cue| lower.contains(cue)) {
        MemoryType::Preference
    } else if TASK_CUES.iter().any(|cue| lower.contains(cue)) {
        MemoryType::Task
    } else {
        MemoryType::Fact
    }
}

/// Extracts discrete typed memories from compacted summary text.
///
/// Pure and deterministic: the same input always yields the same output.
pub fn extract_memories(text: &str) -> Vec<ExtractedMemory> {
    split_sentences(text)
        .into_iter()
        .map(|sentence| {
            let memory_type = classify(&sentence);
            ExtractedMemory {
                title: leading_phrase(&sentence),
                content: sentence.clone(),
                importance: importance_for(memory_type),
                entities: extract_entities(&sentence),
                memory_type,
            }
        })
        .collect()
}

/// Title = the leading phrase of a sentence, capped in length.
fn leading_phrase(sentence: &str) -> String {
    let phrase: Vec<&str> = sentence.split_whitespace().take(10).collect();
    let mut joined = phrase.join(" ");
    if joined.chars().count() > 80 {
        joined = joined.chars().take(77).collect::<String>();
        joined.push_str("...");
    }
    joined
}

/// Importance by cue-word strength: constraints and decisions score highest,
/// notes lowest.
fn importance_for(memory_type: MemoryType) -> u8 {
    match memory_type {
        MemoryType::Constraint => 75,
        MemoryType::Decision => 70,
        MemoryType::Preference => 65,
        MemoryType::Task => 40,
        _ => 50,
    }
}

/// Simple deterministic entity extraction: proper-noun (capitalized) words,
/// camelCase/identifier-like tokens, and backtick-quoted identifiers.
fn extract_entities(sentence: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let words: Vec<&str> = sentence
        .split_whitespace()
        .flat_map(|w| w.split(['(', ')', ',', ':', '.', ';']))
        .collect();

    for (i, raw) in words.iter().enumerate() {
        let word = raw.trim_matches(['`', '"', '\'', '[', ']', '`']);
        if word.chars().count() < 3 {
            continue;
        }
        let is_camel = word.chars().any(|c| c.is_ascii_lowercase())
            && word.chars().any(|c| c.is_ascii_uppercase());
        let is_proper = i > 0
            && word
                .chars()
                .next()
                .map(|c| c.is_ascii_uppercase())
                .unwrap_or(false);
        if is_camel || is_proper {
            found.push(word.to_string());
        }
    }

    normalize_entities(found)
        .into_iter()
        .filter(|e| {
            !e.starts_with("We")
                && !e.starts_with("It")
                && !e.starts_with("The")
                && !e.starts_with("They")
        })
        .collect::<HashSet<_>>()
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_store() -> (Store, TempDir) {
        let tmp = TempDir::new().expect("temp dir");
        let store = Store::open_at(tmp.path().join("locus.db")).expect("store");
        (store, tmp)
    }

    #[test]
    fn adapter_resolves_claude_code_only() {
        assert!(adapter_for("claude-code").is_ok());
        assert!(adapter_for("cursor").is_err());
    }

    #[test]
    fn claude_code_adapter_uses_summary_and_cwd() {
        let payload = r#"{"session_id":"s1","cwd":"/repo/my-app","summary":"We decided to use Postgres for the auth service. Prefer table-driven tests."}"#;
        let trigger = ClaudeCodeAdapter.translate(payload).unwrap();
        assert_eq!(trigger.namespace.as_deref(), Some("project:my-app"));
        assert!(trigger.text.contains("use Postgres"));
    }

    #[test]
    fn claude_code_adapter_accepts_compacted_text_fallback() {
        let payload = r#"{"cwd":"/repo/my-app","compacted_text":"Use sqlite for search"}"#;
        let trigger = ClaudeCodeAdapter.translate(payload).unwrap();
        assert_eq!(trigger.namespace.as_deref(), Some("project:my-app"));
        assert_eq!(trigger.text, "Use sqlite for search");
    }

    #[test]
    fn claude_code_adapter_rejects_missing_summary() {
        let payload = r#"{"cwd":"/repo/my-app"}"#;
        let err = ClaudeCodeAdapter
            .translate(payload)
            .expect_err("missing summary should fail");
        assert!(matches!(err, Error::InvalidInput(_)));
    }

    #[test]
    fn claude_code_adapter_rejects_invalid_payload() {
        let err = ClaudeCodeAdapter
            .translate("not json")
            .expect_err("should fail");
        assert!(matches!(err, Error::InvalidInput(_)));
    }

    #[test]
    fn extraction_produces_typed_memories() {
        let text = "We decided to use Postgres for the auth service. \
                    Prefer table-driven tests. \
                    The service must not expose the internal token. \
                    In progress: wiring the login route.";
        let memories = extract_memories(text);

        let decision = memories
            .iter()
            .find(|m| m.memory_type == MemoryType::Decision)
            .expect("decision memory");
        assert!(decision.content.contains("use Postgres"));
        assert_eq!(decision.importance, 70);

        let preference = memories
            .iter()
            .find(|m| m.memory_type == MemoryType::Preference)
            .expect("preference memory");
        assert!(preference.content.contains("table-driven"));

        let constraint = memories
            .iter()
            .find(|m| m.memory_type == MemoryType::Constraint)
            .expect("constraint memory");
        assert!(constraint.content.contains("must not expose"));
        assert_eq!(constraint.importance, 75);

        let task = memories
            .iter()
            .find(|m| m.memory_type == MemoryType::Task)
            .expect("task memory");
        assert!(task.content.contains("In progress"));
    }

    #[test]
    fn extraction_is_deterministic() {
        let text = "We decided to use Postgres. Prefer explicit errors. Must keep secrets out.";
        let first = extract_memories(text);
        let second = extract_memories(text);
        assert_eq!(first, second);
    }

    #[test]
    fn extraction_skips_short_fragments() {
        let memories = extract_memories("Hi. OK. - Postgres\n\n- Redis\nWe use sqlite for search.");
        assert!(memories.iter().all(|m| !m.content.contains("Hi")));
        assert!(memories
            .iter()
            .any(|m| m.content.contains("use sqlite for search")));
    }

    #[test]
    fn unrelated_sentence_falls_back_to_fact() {
        let memories = extract_memories("The deployment pipeline runs three times daily.");
        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].memory_type, MemoryType::Fact);
    }

    #[test]
    fn capture_writes_namespace_scoped_memories() {
        let (store, _tmp) = test_store();
        let trigger = CaptureTrigger {
            namespace: Some("project:my-app".into()),
            text: "We decided to use Postgres for the auth service.".into(),
        };
        let outcome = capture(&store, &trigger).unwrap();
        assert_eq!(outcome.written, 1);

        let memories = store
            .list_memories(ListFilter {
                namespace: Some("project:my-app".into()),
                ..ListFilter::default()
            })
            .unwrap();
        assert_eq!(memories.len(), 1);
        assert_eq!(memories[0].memory_type, MemoryType::Decision);
        assert!(memories[0].content.contains("use Postgres"));
        assert_eq!(memories[0].source.as_deref(), Some("session:compaction"));
    }

    #[test]
    fn capture_skips_task_state() {
        let (store, _tmp) = test_store();
        let trigger = CaptureTrigger {
            namespace: Some("project:my-app".into()),
            text: "In progress: wiring the login route. We decided to use Postgres.".into(),
        };
        let outcome = capture(&store, &trigger).unwrap();
        assert_eq!(outcome.written, 1);
        assert_eq!(outcome.skipped_tasks, 1);
    }

    #[test]
    fn repeat_capture_does_not_duplicate() {
        let (store, _tmp) = test_store();
        let trigger = CaptureTrigger {
            namespace: Some("project:my-app".into()),
            text: "We decided to use Postgres for the auth service.".into(),
        };
        let first = capture(&store, &trigger).unwrap();
        assert_eq!(first.written, 1);

        let second = capture(&store, &trigger).unwrap();
        assert_eq!(second.written, 0);
        assert_eq!(second.skipped_duplicates, 1);

        let memories = store
            .list_memories(ListFilter {
                namespace: Some("project:my-app".into()),
                ..ListFilter::default()
            })
            .unwrap();
        assert_eq!(memories.len(), 1);
    }

    #[test]
    fn capture_namespaces_do_not_leak() {
        let (store, _tmp) = test_store();
        let trigger = CaptureTrigger {
            namespace: Some("project:my-app".into()),
            text: "We decided to use Postgres for the auth service.".into(),
        };
        capture(&store, &trigger).unwrap();

        let other = store
            .list_memories(ListFilter {
                namespace: Some("project:other".into()),
                ..ListFilter::default()
            })
            .unwrap();
        assert!(other.is_empty(), "capture must not leak across namespaces");
    }

    #[test]
    fn capture_redacts_secrets() {
        let (store, _tmp) = test_store();
        let trigger = CaptureTrigger {
            namespace: Some("project:my-app".into()),
            text: "The deploy token is ghp_123456789012345678901234567890123456 for auth.".into(),
        };
        let outcome = capture(&store, &trigger).unwrap();
        assert_eq!(outcome.written, 1);

        let memories = store
            .list_memories(ListFilter {
                namespace: Some("project:my-app".into()),
                ..ListFilter::default()
            })
            .unwrap();
        assert_eq!(memories.len(), 1);
        assert!(
            !memories[0].content.contains("ghp_1234"),
            "secret must not be stored"
        );
        assert!(memories[0].content.contains("[REDACTED"));
    }

    #[test]
    fn captured_memories_are_retrievable_by_second_agent() {
        let (store, _tmp) = test_store();
        let trigger = CaptureTrigger {
            namespace: Some("project:my-app".into()),
            text: "We decided to use Postgres for the auth service.".into(),
        };
        capture(&store, &trigger).unwrap();

        let query = crate::search::Query {
            text: "Postgres auth".into(),
            namespace: Some("project:my-app".into()),
            memory_type: None,
            limit: 5,
        };
        let outcome = store.retrieve(query).unwrap();
        assert_eq!(outcome.hits.len(), 1);
        assert_eq!(
            outcome.memories[0].title,
            "We decided to use Postgres for the auth service."
        );

        // A second agent retrieves the captured memory through the shared
        // ContextBrief path (U-017 test: "retrievable by a second agent via the
        // shared ContextBrief path").
        let (brief, surfaced) = store
            .context_brief_with_memories(
                crate::search::Query {
                    text: "Postgres auth".into(),
                    namespace: Some("project:my-app".into()),
                    memory_type: None,
                    limit: 5,
                },
                crate::context::ContextBriefOptions::default(),
            )
            .unwrap();
        assert_eq!(surfaced.len(), 1, "captured memory reaches the brief");
        assert!(brief.contains("Postgres"));
    }

    #[test]
    fn capture_extraction_and_write_stay_within_budget() {
        let (store, _tmp) = test_store();
        // Warm the store so the timing reflects steady-state writes, not the
        // one-time schema/migration setup of a fresh database.
        store
            .insert_memory(NewMemory {
                namespace: Some("project:my-app".into()),
                memory_type: MemoryType::Fact,
                title: "warmup".into(),
                content: "warmup write to initialize the schema".into(),
                entities: Vec::new(),
                importance: 1,
                source: None,
            })
            .unwrap();

        let trigger = CaptureTrigger {
            namespace: Some("project:my-app".into()),
            text: ("We decided to use Postgres for the auth service. ".to_owned()
                + "Prefer table-driven tests. "
                + "The DB password must not be committed. "
                + "In progress: wiring the login route. "
                + "Next step: review the migration."),
        };

        let start = std::time::Instant::now();
        capture(&store, &trigger).unwrap();
        let elapsed = start.elapsed();

        // The TECHSTACK budget (< 15ms save p95) is specified for release
        // builds. This debug-mode bound is a regression guard: it must never be
        // anywhere near the cost of an LLM call or network round-trip, which
        // would be seconds, not milliseconds.
        assert!(
            elapsed < std::time::Duration::from_millis(1000),
            "capture took {elapsed:?}; extraction must be local and deterministic"
        );
    }
}
