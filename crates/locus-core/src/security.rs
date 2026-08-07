//! Local secret detection and redaction (U-011).
//!
//! Vendored pattern rules (from gitleaks, see `vendored/`) are compiled once
//! into a process-wide set. The write path uses redact-or-warn semantics:
//! detected secrets are replaced in stored memory with a descriptive
//! `[REDACTED:rule-id]` placeholder and a non-fatal warning is surfaced to the
//! caller. Nothing is silently dropped and nothing is hard-rejected. The
//! original secret value never appears in logs, warnings, or debug output —
//! only the placeholder and the rule id are exposed.

use std::sync::LazyLock;

use regex::Regex;
use serde::Deserialize;

use crate::ipc::protocol::Warning;

/// Text inserted in place of a detected secret. No secret content is kept.
pub const REDACT_PREFIX: &str = "[REDACTED:";

/// One vendored rule as stored in `vendored/gitleaks-subset.toml`.
#[derive(Debug, Clone, Deserialize)]
struct VendoredRule {
    id: String,
    description: String,
    regex: String,
    #[serde(default)]
    entropy: Option<f32>,
    /// Exact-match values that are always treated as safe (example keys from
    /// provider documentation, mirroring gitleaks allowlists).
    #[serde(default)]
    allowlist: Vec<String>,
}

/// Root of the vendored rules document.
#[derive(Debug, Deserialize)]
struct VendoredConfig {
    rules: Vec<VendoredRule>,
}

/// A compiled secret rule.
pub struct SecretRule {
    pub id: String,
    pub description: String,
    pub regex: Regex,
    /// Minimum Shannon entropy (bits/char) required for the match to count.
    pub entropy: Option<f32>,
    /// Compiled allowlist entries; an exact match on the secret value is never
    /// reported.
    pub allowlist: Vec<Regex>,
}

/// Compile the vendored rules exactly once.
static RULES: LazyLock<Vec<SecretRule>> = LazyLock::new(|| {
    let config: VendoredConfig = toml::from_str(include_str!("../vendored/gitleaks-subset.toml"))
        .expect("vendored gitleaks rules must parse");
    config
        .rules
        .into_iter()
        .map(|rule| SecretRule {
            id: rule.id,
            description: rule.description,
            regex: Regex::new(&rule.regex).expect("vendored secret regex must compile"),
            entropy: rule.entropy,
            allowlist: rule
                .allowlist
                .iter()
                .map(|entry| Regex::new(entry).expect("vendored allowlist regex must compile"))
                .collect(),
        })
        .collect()
});

/// The active rule set (for tooling and tests).
pub fn rules() -> &'static [SecretRule] {
    &RULES
}

/// A detected secret inside a scanned string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretMatch {
    /// Rule id that matched (e.g. `aws-access-token`).
    pub rule_id: String,
    /// Byte offset of the match start.
    pub start: usize,
    /// Byte offset of the match end (exclusive).
    pub end: usize,
}

/// Returns the redaction placeholder for a rule id.
pub fn placeholder(rule_id: &str) -> String {
    format!("{REDACT_PREFIX}{rule_id}]")
}

/// Shannon entropy of `text` in bits per character (base-2). Returns 0 for
/// empty input. This is the same measure gitleaks uses for its `entropy`
/// thresholds, so vendored rule thresholds apply unchanged.
fn shannon_entropy(text: &str) -> f64 {
    if text.is_empty() {
        return 0.0;
    }
    let mut counts: std::collections::HashMap<char, usize> = std::collections::HashMap::new();
    let total = text.chars().count();
    for ch in text.chars() {
        *counts.entry(ch).or_insert(0) += 1;
    }
    counts
        .values()
        .map(|count| {
            let p = *count as f64 / total as f64;
            -p * p.log2()
        })
        .sum()
}

/// Scans `text` for secrets and returns matches sorted by start offset.
/// Overlapping matches from different rules are resolved by keeping the
/// leftmost, longest span. Rules with an `entropy` threshold only report
/// matches whose Shannon entropy meets it.
pub fn scan(text: &str) -> Vec<SecretMatch> {
    let mut spans: Vec<(usize, usize, String)> = Vec::new();
    for rule in RULES.iter() {
        for found in rule.regex.find_iter(text) {
            if let Some(min_entropy) = rule.entropy {
                if shannon_entropy(found.as_str()) < f64::from(min_entropy) {
                    continue;
                }
            }
            if rule
                .allowlist
                .iter()
                .any(|entry| entry.is_match(found.as_str()))
            {
                continue;
            }
            spans.push((found.start(), found.end(), rule.id.clone()));
        }
    }

    spans.sort_by_key(|(start, end, _)| (*start, std::cmp::Reverse(*end)));

    let mut out: Vec<SecretMatch> = Vec::new();
    let mut last_end = 0usize;
    for (start, end, rule_id) in spans {
        if start >= last_end {
            last_end = end;
            out.push(SecretMatch {
                rule_id,
                start,
                end,
            });
        }
    }
    out
}

/// True when `text` contains a detected secret.
pub fn contains_secret(text: &str) -> bool {
    !scan(text).is_empty()
}

/// Redacts every detected secret in `text`, replacing it with its placeholder.
/// Returns the redacted text and the matches that were replaced.
pub fn redact(text: &str) -> (String, Vec<SecretMatch>) {
    let matches = scan(text);
    if matches.is_empty() {
        return (text.to_string(), matches);
    }

    let mut out = String::with_capacity(text.len());
    let mut cursor = 0usize;
    for found in &matches {
        out.push_str(&text[cursor..found.start]);
        out.push_str(&placeholder(&found.rule_id));
        cursor = found.end;
    }
    out.push_str(&text[cursor..]);
    (out, matches)
}

/// Redacts secrets across a memory title and content pair (U-011 write path).
/// Returns the redacted title, redacted content, and all matches found.
pub fn redact_title_and_content(title: &str, content: &str) -> (String, String, Vec<SecretMatch>) {
    let (new_title, title_matches) = redact(title);
    let (new_content, content_matches) = redact(content);

    let mut all = title_matches;
    all.extend(content_matches);
    // Sort by start for stable, deterministic warning order.
    all.sort_by_key(|m| (m.start, m.end));

    (new_title, new_content, all)
}

/// Builds a non-fatal, secret-free warning list for a set of matches.
/// The original secret values are never included — only rule ids and counts.
pub fn build_warnings(matches: &[SecretMatch]) -> Vec<Warning> {
    if matches.is_empty() {
        return Vec::new();
    }

    let mut seen: Vec<&str> = Vec::new();
    for found in matches {
        if !seen.iter().any(|id| *id == found.rule_id) {
            seen.push(&found.rule_id);
        }
    }

    let rules = seen.join(", ");
    vec![Warning {
        code: "secret_redacted".to_string(),
        message: format!(
            "redacted {} potential secret(s) from stored memory content; matched rules: {rules}",
            matches.len()
        ),
        field: Some("memory".to_string()),
    }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aws_access_key_is_flagged() {
        assert!(contains_secret("AKIAIOSFODNN7EXAMPLE"));
    }

    #[test]
    fn aws_key_with_embedded_secret_is_flagged() {
        let text = "aws key AKIAIOSFODNN7EXAMPLE stored in the config";
        assert!(contains_secret(text));
        let (redacted, matches) = redact(text);
        assert!(!redacted.contains("AKIAIOSFODNN7EXAMPLE"));
        assert!(redacted.contains("[REDACTED:aws-access-token]"));
        assert_eq!(matches.len(), 1);
    }

    #[test]
    fn github_pat_is_flagged() {
        let token = "ghp_123456789012345678901234567890123456";
        assert!(contains_secret(token));
    }

    #[test]
    fn private_key_block_is_flagged() {
        let key = "-----BEGIN PRIVATE KEY-----\nMIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC7VJTUt9Us8cKjMzE9z3AG7EHjA3H8jFmC9f9H9yS3w8ABC\n-----END PRIVATE KEY-----";
        assert!(contains_secret(key));
    }

    #[test]
    fn password_in_url_is_flagged() {
        assert!(contains_secret("https://user:secret123@example.com/path"));
        assert!(contains_secret(
            "postgres://admin:hunter2@db.example.com:5432/app"
        ));
    }

    #[test]
    fn uuid_is_not_flagged() {
        assert!(!contains_secret("550e8400-e29b-41d4-a716-446655440000"));
    }

    #[test]
    fn git_commit_sha_is_not_flagged() {
        assert!(!contains_secret("fe2a5c9b7a3d1f0e8c6b4a2d9e7f1c3b5a8d4e6f"));
    }

    #[test]
    fn dependency_lock_hash_is_not_flagged() {
        let npm_integrity =
            "sha512-Ya2j6hBd0P0CezEYo3RGhGyGLjKvSjOPSQyIBK3yHQV3IfLMJNw0VnLoM+7F3Pn8";
        assert!(!contains_secret(npm_integrity));
        let pypi = "sha256=5a94d0a6d5d52a8f88b2f0c1f1b6e5a94d0a6d5d52a8f88b2f0c1f1b6e5a94";
        assert!(!contains_secret(pypi));
    }

    #[test]
    fn long_benign_base64_is_not_flagged() {
        let blob = "Q29tZW50cyBhcmUgd2VsbGNvbWUgaW4gdGhpcyBwcm9qZWN0IGFuZCB0aGlzIGlzIGp1c3QgYSByZWd1bGFyIGJhc2U2NCBlbmNvZGVkIHN0cmluZyB0aGF0IGxvb2tzIGxpa2UgYSB0b2tlbiBidXQgaXMgbm90Li4u";
        assert!(!contains_secret(blob));
    }

    #[test]
    fn redaction_never_keeps_secret_value() {
        let token = "ghp_123456789012345678901234567890123456";
        let text = format!("deploy token is {token} on the box");
        let (redacted, matches) = redact(&text);
        assert_eq!(matches.len(), 1);
        assert!(!redacted.contains(token));
        assert!(redacted.contains("[REDACTED:github-pat]"));
        // Warnings must also be secret-free.
        let warnings = build_warnings(&matches);
        for warning in &warnings {
            assert!(
                !warning.message.contains(token),
                "warning leaked the secret"
            );
        }
    }

    #[test]
    fn matches_are_sorted_by_start_offset_and_non_overlapping() {
        let text = "key AKIAIOSFODNN7EXAMPLE and ghp_123456789012345678901234567890123456";
        let matches = scan(text);
        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0].rule_id, "aws-access-token");
        assert_eq!(matches[1].rule_id, "github-pat");
        assert!(matches[0].start < matches[1].start);
        assert!(matches[0].end <= matches[1].start, "spans must not overlap");
    }

    #[test]
    fn redact_title_and_content_spans_both_fields() {
        let (title, content, matches) = redact_title_and_content(
            "key AKIAIOSFODNN7EXAMPLE",
            "see ghp_123456789012345678901234567890123456",
        );
        assert!(title.contains("[REDACTED:aws-access-token]"));
        assert!(content.contains("[REDACTED:github-pat]"));
        assert_eq!(matches.len(), 2);
    }

    #[test]
    fn build_warnings_dedupes_by_rule() {
        let (_, _, matches) = redact_title_and_content(
            "AKIAIOSFODNN7EXAMPLE",
            "AKIAIOSFODNN7EXAMPLE again ghp_123456789012345678901234567890123456",
        );
        let warnings = build_warnings(&matches);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].code, "secret_redacted");
        assert!(warnings[0].message.contains("3"));
    }

    #[test]
    fn gcp_example_key_from_docs_is_not_flagged() {
        // This exact key appears in Google's public documentation and is on the
        // gitleaks allowlist; it must not be redacted.
        assert!(!contains_secret("AIzaSyDMAScliyLx7F0NPDEJi1QmyCgHIAODrlU"));
    }

    #[test]
    fn vendored_rules_compile_and_are_nonempty() {
        assert!(!rules().is_empty());
        for rule in rules() {
            assert!(!rule.id.is_empty());
            assert!(!rule.regex.as_str().is_empty());
        }
    }
}
