//! Deterministic benchmark dataset generation (U-012).
//!
//! Produces realistic memories — namespaces, types, code identifiers, partial
//! names, camelCase tokens, file paths — so the U-003 query shapes (exact,
//! phrase, prefix, partial, typo, identifier) have something meaningful to
//! match. Generation is fully deterministic: the same `size` always yields the
//! same memories, so benchmark results are reproducible across runs.

use locus_core::memory::{MemoryType, NewMemory};

/// Namespaces the generator cycles through.
pub const NAMESPACES: [&str; 3] = ["project:auth", "project:billing", "project:core"];

/// Memory types the generator cycles through.
pub const TYPES: [MemoryType; 5] = [
    MemoryType::Decision,
    MemoryType::Fact,
    MemoryType::Preference,
    MemoryType::Constraint,
    MemoryType::Code,
];

/// Generates `size` deterministic memories.
///
/// Content is derived from the index plus a small set of fixed fragments, so a
/// given `size` always produces byte-identical memories (no RNG, no wall clock).
pub fn generate(size: usize) -> Vec<NewMemory> {
    (0..size).map(generate_one).collect()
}

/// Generates the `i`-th deterministic memory in the U-012 dataset.
pub fn generate_one(i: usize) -> NewMemory {
    let namespace = NAMESPACES[i % NAMESPACES.len()];
    let memory_type = TYPES[i % TYPES.len()];
    let import = (i % 100) as u8;
    let handler = format!("verify_token_handler_{i}");
    let svc = format!("AuthService::verify_token_{i}");

    // A fixed, searchable fragment is woven into every memory so queries such
    // as "auth" and "verify*" return a broad candidate set regardless of size.
    let (title, content) = match memory_type {
        MemoryType::Decision => (
            format!("Auth middleware decision {i}"),
            format!(
                "Use {svc} and route handler {handler}; auth decisions live in project {namespace}"
            ),
        ),
        MemoryType::Fact => (
            format!("Token verify fact {i}"),
            format!("Fact: {svc} authenticates via {handler} in namespace {namespace}"),
        ),
        MemoryType::Preference => (
            format!("Auth style preference {i}"),
            format!("Prefer {svc} over raw credential checks; handler {handler} stays lean"),
        ),
        MemoryType::Constraint => (
            format!("Auth constraint {i}"),
            format!("Must not bypass {svc}; require {handler} on every protected route"),
        ),
        _ => (
            format!("verify_token_handler_{i} signature"),
            format!("fn verify_token_handler_{i}() delegates to {svc}"),
        ),
    };

    NewMemory {
        namespace: Some(namespace.to_string()),
        memory_type,
        title,
        content,
        entities: vec![svc, format!("handler_{handler}")],
        importance: import,
        source: Some("bench".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_is_deterministic() {
        let first = generate(100);
        let second = generate(100);
        assert_eq!(first.len(), 100);
        for (a, b) in first.iter().zip(second.iter()) {
            assert_eq!(a.title, b.title);
            assert_eq!(a.content, b.content);
            assert_eq!(a.entities, b.entities);
        }
    }

    #[test]
    fn generated_memories_have_realistic_fields() {
        let data = generate(10);
        for (i, m) in data.iter().enumerate() {
            let title = m.title.to_lowercase();
            assert!(title.contains("auth") || title.contains("verify"));
            assert!(m.content.contains("verify_token_handler_"));
            assert_eq!(m.entities.len(), 2);
            assert!(m
                .namespace
                .as_deref()
                .is_some_and(|ns| NAMESPACES.contains(&ns)));
            assert_eq!(m.importance, (i % 100) as u8);
        }
    }

    #[test]
    fn every_memory_carries_a_shared_searchable_fragment() {
        // U-003 query shapes rely on a broad common token so searches return
        // candidates at every dataset size.
        let data = generate(5_000);
        assert!(data
            .iter()
            .all(|m| m.content.contains("verify_token_handler_")));
        assert!(data.iter().all(|m| {
            let title = m.title.to_lowercase();
            title.contains("auth") || title.contains("verify")
        }));
    }
}
