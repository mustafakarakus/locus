//! Context brief generation from relevant memories.

use std::collections::{HashMap, HashSet};

use crate::memory::{Memory, MemoryType};

pub const NO_RELEVANT_MEMORY: &str = "NO_RELEVANT_MEMORY";
const DEFAULT_TOKEN_BUDGET: usize = 400;

#[derive(Debug, Clone, Copy)]
pub struct ContextBriefOptions {
    pub token_budget: usize,
}

impl Default for ContextBriefOptions {
    fn default() -> Self {
        Self {
            token_budget: DEFAULT_TOKEN_BUDGET,
        }
    }
}

#[derive(Debug, Clone)]
struct BriefItem {
    id: String,
    category: Category,
    text: String,
    normalized: String,
    updated_at: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Category {
    Decisions,
    Preferences,
    Constraints,
    Tasks,
}

impl Category {
    fn heading(self) -> &'static str {
        match self {
            Self::Decisions => "## Decisions",
            Self::Preferences => "## Preferences",
            Self::Constraints => "## Constraints",
            Self::Tasks => "## Tasks",
        }
    }

    fn order(self) -> usize {
        match self {
            Self::Decisions => 0,
            Self::Preferences => 1,
            Self::Constraints => 2,
            Self::Tasks => 3,
        }
    }
}

pub fn build_context_brief(memories: &[Memory], options: ContextBriefOptions) -> String {
    build_context_brief_with_selected(memories, options).0
}

/// Like [`build_context_brief`], but also returns the memories that made it
/// into the final brief (after dedupe and budget capping). Used by the daemon
/// to record access and emit live events for exactly what was surfaced
/// (U-016).
pub fn build_context_brief_with_selected(
    memories: &[Memory],
    options: ContextBriefOptions,
) -> (String, Vec<&Memory>) {
    let budget = options.token_budget.max(1);
    if memories.is_empty() {
        return (NO_RELEVANT_MEMORY.to_string(), Vec::new());
    }

    let mut items = memories.iter().map(memory_to_item).collect::<Vec<_>>();

    dedupe_items(&mut items);
    if items.is_empty() {
        return (NO_RELEVANT_MEMORY.to_string(), Vec::new());
    }

    items.sort_by(|left, right| {
        left.category
            .order()
            .cmp(&right.category.order())
            .then_with(|| right.updated_at.cmp(&left.updated_at))
            .then_with(|| left.id.cmp(&right.id))
    });

    let (brief, selected_items) = render_with_budget(&items, budget);
    let selected = if brief == NO_RELEVANT_MEMORY {
        Vec::new()
    } else {
        selected_items
            .into_iter()
            .filter_map(|item| memories.iter().find(|memory| memory.id == item.id))
            .collect()
    };
    (brief, selected)
}

pub fn estimated_tokens(markdown: &str) -> usize {
    if markdown.trim().is_empty() {
        return 0;
    }

    // Practical approximation for compact markdown: ~0.75 words per token.
    let words = markdown.split_whitespace().count();
    words.saturating_mul(4).div_ceil(3)
}

fn memory_to_item(memory: &Memory) -> BriefItem {
    let category = category_for(memory.memory_type);
    let text = bullet_text(memory);
    let normalized = normalize_for_dedupe(&text);

    BriefItem {
        id: memory.id.clone(),
        category,
        text,
        normalized,
        updated_at: memory.updated_at,
    }
}

fn category_for(memory_type: MemoryType) -> Category {
    match memory_type {
        MemoryType::Decision => Category::Decisions,
        MemoryType::Preference => Category::Preferences,
        MemoryType::Constraint => Category::Constraints,
        MemoryType::Task => Category::Tasks,
        MemoryType::Fact
        | MemoryType::Bug
        | MemoryType::Architecture
        | MemoryType::Code
        | MemoryType::Note => Category::Constraints,
    }
}

fn bullet_text(memory: &Memory) -> String {
    let title = memory.title.trim();
    let content = memory.content.trim();
    if title.is_empty() {
        clip_chars(content, 180)
    } else if content.is_empty() {
        clip_chars(title, 180)
    } else {
        format!("{}: {}", clip_chars(title, 80), clip_chars(content, 180))
    }
}

fn clip_chars(input: &str, max_chars: usize) -> String {
    if input.chars().count() <= max_chars {
        return input.to_string();
    }

    let clipped = input
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    format!("{}...", clipped)
}

pub(crate) fn normalize_for_dedupe(input: &str) -> String {
    input
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() {
                ch.to_ascii_lowercase()
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn dedupe_items(items: &mut Vec<BriefItem>) {
    items.sort_by(|left, right| {
        right
            .updated_at
            .cmp(&left.updated_at)
            .then_with(|| left.id.cmp(&right.id))
    });

    let mut kept = Vec::with_capacity(items.len());
    let mut exact = HashSet::new();

    for item in items.iter() {
        if item.normalized.is_empty() {
            continue;
        }

        if !exact.insert(item.normalized.clone()) {
            continue;
        }

        // Keep near-duplicates out when one normalized string mostly contains the other.
        let is_near_duplicate = kept
            .iter()
            .any(|existing: &BriefItem| near_duplicate(&item.normalized, &existing.normalized));

        if !is_near_duplicate {
            kept.push(item.clone());
        }
    }

    *items = kept;
}

pub(crate) fn near_duplicate(left: &str, right: &str) -> bool {
    if left == right {
        return true;
    }

    let (short, long) = if left.len() <= right.len() {
        (left, right)
    } else {
        (right, left)
    };

    if short.len() < 24 {
        return false;
    }

    if long.contains(short) {
        let ratio = short.len() as f32 / long.len() as f32;
        return ratio >= 0.75;
    }

    false
}

fn render_with_budget(items: &[BriefItem], token_budget: usize) -> (String, Vec<&BriefItem>) {
    let categories = [
        Category::Decisions,
        Category::Preferences,
        Category::Constraints,
        Category::Tasks,
    ];

    let mut selected_by_category: HashMap<Category, Vec<&str>> = HashMap::new();
    let mut selected: Vec<&BriefItem> = Vec::new();

    for item in items {
        selected_by_category
            .entry(item.category)
            .or_default()
            .push(item.text.as_str());
        selected.push(item);

        let rendered = render_markdown(&categories, &selected_by_category);
        if estimated_tokens(&rendered) > token_budget {
            if let Some(bucket) = selected_by_category.get_mut(&item.category) {
                bucket.pop();
            }
            selected.pop();
            break;
        }
    }

    let markdown = render_markdown(&categories, &selected_by_category);
    if selected_by_category.values().all(Vec::is_empty) {
        (NO_RELEVANT_MEMORY.to_string(), Vec::new())
    } else {
        (markdown, selected)
    }
}

fn render_markdown(categories: &[Category], selected: &HashMap<Category, Vec<&str>>) -> String {
    let mut lines = vec!["# Locus Memory Brief".to_string()];

    for category in categories {
        let items = selected.get(category).map(Vec::as_slice).unwrap_or(&[]);
        if items.is_empty() {
            continue;
        }

        lines.push(String::new());
        lines.push(category.heading().to_string());
        for item in items {
            lines.push(format!("- {}", item));
        }
    }

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_memory(
        id: &str,
        memory_type: MemoryType,
        title: &str,
        content: &str,
        updated_at: i64,
    ) -> Memory {
        Memory {
            id: id.to_string(),
            namespace: "project:auth".to_string(),
            memory_type,
            title: title.to_string(),
            content: content.to_string(),
            entities: vec![],
            importance: 50,
            source: None,
            created_at: updated_at,
            updated_at,
            access_count: 0,
            last_accessed_at: None,
        }
    }

    #[test]
    fn empty_memory_list_returns_no_relevant_memory() {
        let brief = build_context_brief(&[], ContextBriefOptions::default());
        assert_eq!(brief, NO_RELEVANT_MEMORY);
    }

    #[test]
    fn decisions_and_preferences_are_grouped_into_expected_headings() {
        let memories = vec![
            sample_memory(
                "1",
                MemoryType::Decision,
                "Database choice",
                "Use Postgres for auth service",
                10,
            ),
            sample_memory(
                "2",
                MemoryType::Preference,
                "Testing style",
                "Prefer table-driven tests",
                20,
            ),
        ];

        let brief = build_context_brief(&memories, ContextBriefOptions::default());
        assert!(brief.contains("## Decisions"));
        assert!(brief.contains("## Preferences"));
    }

    #[test]
    fn output_stays_within_token_budget() {
        let memories = vec![
            sample_memory(
                "1",
                MemoryType::Decision,
                "A very long decision title that still needs clipping to preserve budgets",
                "This decision body is intentionally verbose so the generated output consumes noticeable space and exercises token capping logic in the brief renderer.",
                10,
            ),
            sample_memory(
                "2",
                MemoryType::Task,
                "Implement integration",
                "Add daemon and cli integration tests once context brief is stable and deterministic.",
                9,
            ),
        ];

        let brief = build_context_brief(&memories, ContextBriefOptions { token_budget: 35 });
        assert!(estimated_tokens(&brief) <= 35);
    }

    #[test]
    fn duplicates_are_merged() {
        let memories = vec![
            sample_memory(
                "1",
                MemoryType::Decision,
                "Auth DB",
                "Use Postgres for auth service",
                20,
            ),
            sample_memory(
                "2",
                MemoryType::Decision,
                "Auth DB",
                "Use Postgres for auth service",
                10,
            ),
        ];

        let brief = build_context_brief(&memories, ContextBriefOptions::default());
        let count = brief.matches("Auth DB").count();
        assert_eq!(count, 1);
    }

    #[test]
    fn output_is_valid_markdown_shape() {
        let memories = vec![sample_memory(
            "1",
            MemoryType::Task,
            "Follow-up",
            "Measure context brief latency",
            30,
        )];

        let brief = build_context_brief(&memories, ContextBriefOptions::default());
        assert!(brief.starts_with("# Locus Memory Brief"));
        assert!(brief.contains("## Tasks"));
        assert!(brief.contains("- Follow-up: Measure context brief latency"));
    }

    #[test]
    fn output_is_deterministic_for_same_input() {
        let memories = vec![
            sample_memory(
                "b",
                MemoryType::Preference,
                "Style",
                "Prefer explicit error messages",
                100,
            ),
            sample_memory("a", MemoryType::Decision, "Store", "Use sqlite fts5", 200),
        ];

        let first = build_context_brief(&memories, ContextBriefOptions::default());
        let second = build_context_brief(&memories, ContextBriefOptions::default());
        assert_eq!(first, second);
    }
}
