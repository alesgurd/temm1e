//! # Wish-pattern detector (Wire 5).
//!
//! Scans a sequence of user messages for patterns that indicate an unmet
//! capability need. When the same pattern repeats, emit a suggestion that
//! the user consider running `/cambium grow <X>`.
//!
//! This is a lightweight, LLM-free pattern matcher designed to run on
//! every agent turn with minimal overhead. When it detects a pattern it
//! returns a `WishSuggestion` that the caller (agent runtime, anima
//! evaluator, or slash command) can surface to the user.
//!
//! ## Why keyword matching, not LLM?
//!
//! - Zero cost (no API call per turn).
//! - Deterministic and testable.
//! - Runs frequently without budget impact.
//! - The pattern list is narrow and specific — false positives are OK
//!   because the output is a suggestion, not an action.

use serde::{Deserialize, Serialize};

/// A single detected wish-pattern match.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WishMatch {
    /// The message that triggered the match (trimmed).
    pub message: String,
    /// The extracted wish text (what the user wants).
    pub wish: String,
}

/// A suggestion that the caller should surface to the user.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WishSuggestion {
    /// Normalized wish text (lowercased, whitespace-collapsed).
    pub normalized: String,
    /// How many times this exact wish has been seen.
    pub count: usize,
    /// All the raw messages that matched.
    pub matches: Vec<WishMatch>,
}

/// Prefixes that indicate a wish. We keep this narrow to avoid false
/// positives. Case-insensitive matching.
const WISH_PREFIXES: &[&str] = &[
    "i wish you could",
    "i wish you would",
    "it would be nice if you could",
    "it would be nice if you",
    "can you please",
    "why can't you",
    "you can't",
    "you still can't",
    "why don't you",
    "if only you could",
    "i wish tem could",
    "tem should be able to",
];

/// Extract a wish from a single message. Returns `None` if no known
/// wish-prefix is present.
pub fn extract_wish(message: &str) -> Option<WishMatch> {
    let trimmed = message.trim();
    if trimmed.is_empty() {
        return None;
    }
    let lower = trimmed.to_lowercase();
    for prefix in WISH_PREFIXES {
        if let Some(rest) = lower.strip_prefix(prefix) {
            let wish = rest
                .trim_start_matches(|c: char| c.is_whitespace() || c == ',' || c == ':')
                .trim_end_matches(|c: char| c == '.' || c == '?' || c == '!' || c.is_whitespace())
                .to_string();
            if wish.is_empty() || wish.len() < 3 {
                return None;
            }
            return Some(WishMatch {
                message: trimmed.to_string(),
                wish,
            });
        }
    }
    None
}

/// Collapse whitespace and lowercase a string so that "monitor my  K8s"
/// and "monitor my k8s" count as the same wish.
fn normalize_wish(wish: &str) -> String {
    wish.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// Scan a sequence of messages (most recent first or last — order doesn't
/// matter) and return any wishes that repeat `min_count` or more times.
pub fn find_repeated_wishes(messages: &[&str], min_count: usize) -> Vec<WishSuggestion> {
    let mut by_norm: std::collections::HashMap<String, Vec<WishMatch>> =
        std::collections::HashMap::new();

    for msg in messages {
        if let Some(m) = extract_wish(msg) {
            let norm = normalize_wish(&m.wish);
            by_norm.entry(norm).or_default().push(m);
        }
    }

    let mut out: Vec<WishSuggestion> = by_norm
        .into_iter()
        .filter(|(_, v)| v.len() >= min_count)
        .map(|(norm, matches)| WishSuggestion {
            normalized: norm,
            count: matches.len(),
            matches,
        })
        .collect();

    // Sort by count desc, then by normalized text for stability.
    out.sort_by(|a, b| {
        b.count
            .cmp(&a.count)
            .then_with(|| a.normalized.cmp(&b.normalized))
    });
    out
}

/// Format a suggestion as a user-facing message. The caller can decide
/// whether to prepend this to a chat reply, store it, or ignore it.
pub fn format_suggestion(suggestion: &WishSuggestion) -> String {
    format!(
        "Cambium noticed you've asked about this {} times: \"{}\".\n\
         Want me to try growing this capability? Run:\n\
         /cambium grow {}",
        suggestion.count, suggestion.normalized, suggestion.normalized
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_wish_simple() {
        let m = extract_wish("I wish you could monitor my Kubernetes pods").unwrap();
        assert!(m.wish.contains("monitor"));
        assert!(m.wish.contains("kubernetes"));
    }

    #[test]
    fn extract_wish_case_insensitive() {
        let m = extract_wish("I WISH YOU COULD ping a URL").unwrap();
        assert!(m.wish.contains("ping"));
    }

    #[test]
    fn extract_wish_strips_trailing_punctuation() {
        let m = extract_wish("I wish you could read PDFs.").unwrap();
        assert!(!m.wish.ends_with('.'));
    }

    #[test]
    fn extract_wish_no_match() {
        assert!(extract_wish("hello there").is_none());
        assert!(extract_wish("the weather is nice").is_none());
    }

    #[test]
    fn extract_wish_empty_message() {
        assert!(extract_wish("").is_none());
        assert!(extract_wish("   ").is_none());
    }

    #[test]
    fn extract_wish_too_short() {
        // "I wish you could do" — "do" is 2 chars, below min
        assert!(extract_wish("I wish you could do").is_none());
    }

    #[test]
    fn extract_wish_multiple_prefixes() {
        let m = extract_wish("can you please read my calendar").unwrap();
        assert!(m.wish.contains("calendar"));
    }

    #[test]
    fn extract_wish_why_cant() {
        let m = extract_wish("Why can't you parse JSON from a URL?").unwrap();
        assert!(m.wish.to_lowercase().contains("parse"));
    }

    #[test]
    fn normalize_wish_collapses_whitespace() {
        assert_eq!(normalize_wish("foo   bar"), "foo bar");
        assert_eq!(normalize_wish("Foo Bar"), "foo bar");
        assert_eq!(normalize_wish("  x  y  "), "x y");
    }

    #[test]
    fn find_repeated_wishes_single_occurrence() {
        let msgs = vec!["I wish you could read PDFs"];
        let out = find_repeated_wishes(&msgs, 2);
        assert!(out.is_empty(), "single match should not trigger");
    }

    #[test]
    fn find_repeated_wishes_three_occurrences() {
        let msgs = vec![
            "I wish you could read PDFs",
            "Can you please read PDFs",
            "Why can't you read PDFs",
        ];
        let out = find_repeated_wishes(&msgs, 3);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].count, 3);
        assert!(out[0].normalized.contains("read pdfs"));
    }

    #[test]
    fn find_repeated_wishes_mixed() {
        let msgs = vec![
            "I wish you could monitor k8s",
            "I wish you could monitor k8s",
            "I wish you could monitor k8s",
            "I wish you could read emails", // only once
            "hello world",                  // not a wish
        ];
        let out = find_repeated_wishes(&msgs, 3);
        assert_eq!(out.len(), 1);
        assert!(out[0].normalized.contains("monitor k8s"));
    }

    #[test]
    fn find_repeated_wishes_sorts_by_count() {
        let msgs = vec![
            "I wish you could alpha the system",
            "I wish you could bravo the pipeline",
            "I wish you could bravo the pipeline",
            "I wish you could bravo the pipeline",
            "I wish you could charlie the code",
            "I wish you could charlie the code",
        ];
        let out = find_repeated_wishes(&msgs, 2);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].count, 3); // bravo wins
        assert_eq!(out[1].count, 2); // charlie second
    }

    #[test]
    fn format_suggestion_mentions_command() {
        let s = WishSuggestion {
            normalized: "ping a url".into(),
            count: 4,
            matches: vec![],
        };
        let formatted = format_suggestion(&s);
        assert!(formatted.contains("/cambium grow"));
        assert!(formatted.contains("4"));
        assert!(formatted.contains("ping a url"));
    }

    #[test]
    fn wishmatch_serializes() {
        let m = WishMatch {
            message: "msg".into(),
            wish: "wish".into(),
        };
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains("msg"));
    }
}
