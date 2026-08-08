//! Conflict detection: identifies memories that likely contradict each other.
//!
//! Two memories are considered a potential conflict when they share the same
//! namespace and type and their titles overlap on at least two significant
//! (non-trivial) keywords. Only conflict-eligible types are checked:
//! `Decision`, `Architecture`, and `Preference`.

use serde::{Deserialize, Serialize};

use crate::memory::MemoryType;

/// Memory types eligible for automatic conflict detection.
pub const CONFLICT_ELIGIBLE_TYPES: &[MemoryType] = &[
    MemoryType::Decision,
    MemoryType::Architecture,
    MemoryType::Preference,
    MemoryType::Constraint,
];

/// Minimum number of shared significant words required to flag two memories as
/// a potential conflict.
pub const MIN_SHARED_WORDS: usize = 2;

/// Short or semantically empty words that do not carry topic signal.
const STOP_WORDS: &[&str] = &[
    "use", "uses", "used", "using", "the", "this", "that", "these", "those", "and", "or", "not",
    "but", "for", "with", "without", "is", "are", "was", "were", "been", "in", "on", "at", "by",
    "to", "of", "a", "an", "all", "any", "over", "than", "more", "should", "must", "will", "can",
    "does", "from", "into", "when", "where", "how", "have", "has", "had", "their", "they", "them",
    "each", "also", "only",
];

/// A persisted record linking two memories that potentially contradict each
/// other.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConflictRecord {
    /// Autoincrement row id from the `memory_conflicts` table.
    pub id: i64,
    /// The lexicographically earlier of the two conflicting memory ids.
    pub memory_id_a: String,
    /// The lexicographically later of the two conflicting memory ids.
    pub memory_id_b: String,
    /// Human-readable description of why the conflict was detected.
    pub reason: String,
    /// Unix timestamp (seconds) when the conflict was recorded.
    pub detected_at: i64,
    /// Unix timestamp when the conflict was resolved, or `None` if it is still
    /// open.
    pub resolved_at: Option<i64>,
}

/// Returns `true` if the given type is eligible for automatic conflict
/// detection.
pub fn is_conflict_eligible(memory_type: MemoryType) -> bool {
    CONFLICT_ELIGIBLE_TYPES.contains(&memory_type)
}

/// Extracts significant (non-trivial, non-short) words from `text`.
///
/// Words shorter than 4 characters and words present in [`STOP_WORDS`] are
/// excluded. The returned words are lowercase and in order of appearance.
pub fn significant_words(text: &str) -> Vec<String> {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.to_lowercase())
        .filter(|w| w.len() >= 4 && !STOP_WORDS.contains(&w.as_str()))
        .collect()
}

/// Returns the number of significant words shared between `title_a` and
/// `title_b`.
pub fn shared_word_count(title_a: &str, title_b: &str) -> usize {
    let words_a = significant_words(title_a);
    let words_b = significant_words(title_b);
    words_a.iter().filter(|w| words_b.contains(w)).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn significant_words_filters_stop_words_and_short_words() {
        let words = significant_words("Use Postgres for auth service");
        assert!(!words.contains(&"use".to_string()));
        assert!(!words.contains(&"for".to_string()));
        assert!(words.contains(&"postgres".to_string()));
        assert!(words.contains(&"auth".to_string()));
        assert!(words.contains(&"service".to_string()));
    }

    #[test]
    fn significant_words_filters_words_shorter_than_four_chars() {
        let words = significant_words("use JWT for API");
        // "JWT" → "jwt" is 3 chars, filtered
        // "API" → "api" is 3 chars, filtered
        assert!(!words.contains(&"jwt".to_string()));
        assert!(!words.contains(&"api".to_string()));
    }

    #[test]
    fn shared_word_count_finds_overlap() {
        let count = shared_word_count(
            "Use Postgres for auth service",
            "Prefer Postgres auth backend",
        );
        assert!(count >= 2, "expected at least 2 shared words, got {count}");
    }

    #[test]
    fn shared_word_count_no_overlap() {
        let count = shared_word_count("Use Postgres for auth", "Switch to Redis for caching");
        assert_eq!(count, 0);
    }

    #[test]
    fn conflict_eligible_types() {
        assert!(is_conflict_eligible(MemoryType::Decision));
        assert!(is_conflict_eligible(MemoryType::Architecture));
        assert!(is_conflict_eligible(MemoryType::Preference));
        assert!(!is_conflict_eligible(MemoryType::Task));
        assert!(!is_conflict_eligible(MemoryType::Bug));
        assert!(!is_conflict_eligible(MemoryType::Code));
        assert!(!is_conflict_eligible(MemoryType::Note));
        assert!(!is_conflict_eligible(MemoryType::Fact));
    }
}
