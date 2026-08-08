//! Canonical memory data model and validation utilities.

use crate::{Error, Result};

/// Supported memory categories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemoryType {
    Fact,
    Decision,
    Preference,
    Constraint,
    Task,
    Bug,
    Architecture,
    Code,
    Note,
}

impl MemoryType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Fact => "fact",
            Self::Decision => "decision",
            Self::Preference => "preference",
            Self::Constraint => "constraint",
            Self::Task => "task",
            Self::Bug => "bug",
            Self::Architecture => "architecture",
            Self::Code => "code",
            Self::Note => "note",
        }
    }

    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "fact" => Ok(Self::Fact),
            "decision" => Ok(Self::Decision),
            "preference" => Ok(Self::Preference),
            "constraint" => Ok(Self::Constraint),
            "task" => Ok(Self::Task),
            "bug" => Ok(Self::Bug),
            "architecture" => Ok(Self::Architecture),
            "code" => Ok(Self::Code),
            "note" => Ok(Self::Note),
            _ => Err(Error::InvalidInput(format!("invalid memory type: {value}"))),
        }
    }
}

/// The canonical persisted memory object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Memory {
    pub id: String,
    pub namespace: String,
    pub memory_type: MemoryType,
    pub title: String,
    pub content: String,
    pub entities: Vec<String>,
    pub importance: u8,
    pub source: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
    /// Number of times this memory has been surfaced to a caller (U-016).
    pub access_count: u64,
    /// Last time this memory was surfaced to a caller, if ever (U-016).
    pub last_accessed_at: Option<i64>,
}

/// Input for creating a new memory.
#[derive(Debug, Clone)]
pub struct NewMemory {
    pub namespace: Option<String>,
    pub memory_type: MemoryType,
    pub title: String,
    pub content: String,
    pub entities: Vec<String>,
    pub importance: u8,
    pub source: Option<String>,
}

/// Input for updating an existing memory.
#[derive(Debug, Clone)]
pub struct UpdateMemory {
    pub id: String,
    pub namespace: Option<String>,
    pub memory_type: MemoryType,
    pub title: String,
    pub content: String,
    pub entities: Vec<String>,
    pub importance: u8,
    pub source: Option<String>,
}

/// Query filter for listing memories.
#[derive(Debug, Clone, Default)]
pub struct ListFilter {
    pub namespace: Option<String>,
    pub memory_type: Option<MemoryType>,
    pub limit: Option<usize>,
}

pub(crate) fn normalize_namespace(raw: Option<String>) -> String {
    match raw {
        Some(ns) if !ns.trim().is_empty() => ns.trim().to_string(),
        _ => "global".to_string(),
    }
}

pub(crate) fn normalize_optional(raw: Option<String>) -> Option<String> {
    raw.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

pub(crate) fn normalize_entities(entities: Vec<String>) -> Vec<String> {
    let mut normalized = Vec::new();
    for item in entities {
        let trimmed = item.trim();
        if trimmed.is_empty() {
            continue;
        }
        if !normalized.iter().any(|existing| existing == trimmed) {
            normalized.push(trimmed.to_string());
        }
    }
    normalized
}

pub(crate) fn validate_title(title: &str) -> Result<()> {
    if title.trim().is_empty() {
        return Err(Error::InvalidInput("title must not be empty".to_string()));
    }
    Ok(())
}

pub(crate) fn validate_content(content: &str) -> Result<()> {
    if content.trim().is_empty() {
        return Err(Error::InvalidInput("content must not be empty".to_string()));
    }
    Ok(())
}

pub(crate) fn validate_importance(importance: u8) -> Result<()> {
    if importance > 100 {
        return Err(Error::InvalidInput(
            "importance must be between 0 and 100".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_invalid_memory_type_is_rejected() {
        let err = MemoryType::parse("invalid").expect_err("invalid type should error");
        assert!(matches!(err, Error::InvalidInput(_)));
    }

    #[test]
    fn namespace_defaults_to_global() {
        assert_eq!(normalize_namespace(None), "global");
        assert_eq!(normalize_namespace(Some("".to_string())), "global");
    }
}
