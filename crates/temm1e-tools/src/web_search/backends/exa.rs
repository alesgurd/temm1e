//! Exa neural search API (paid, opt-in).
//!
//! Endpoint: https://api.exa.ai/search (POST)
//! Auth: `x-api-key: $EXA_API_KEY` header.
//! Auto-registered but disabled unless `EXA_API_KEY` env var is set.
//!
//! This is the rewritten, trait-shaped version of PR #42's web_search.rs.
//! See docs/web_search/RESEARCH.md for the current (2026-04-12) API shape.

use super::{fetch_bounded, make_client, truncate_safe, SearchBackend};
use crate::web_search::types::*;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

pub struct ExaBackend {
    client: reqwest::Client,
    api_key: Option<String>,
}

impl ExaBackend {
    pub fn new() -> Self {
        let api_key = std::env::var("EXA_API_KEY").ok().filter(|s| !s.is_empty());
        Self {
            client: make_client(),
            api_key,
        }
    }
}

impl Default for ExaBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Deserialize)]
struct ApiResponse {
    #[serde(default)]
    results: Vec<ExaResult>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct ExaResult {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    title: Option<String>,
    url: String,
    #[serde(default, rename = "publishedDate")]
    published_date: Option<String>,
    #[serde(default)]
    author: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    highlights: Option<Vec<String>>,
    #[serde(default)]
    summary: Option<String>,
}

#[async_trait]
impl SearchBackend for ExaBackend {
    fn id(&self) -> BackendId {
        BackendId::Exa
    }
    fn name(&self) -> &str {
        "exa"
    }
    fn enabled(&self) -> bool {
        self.api_key.is_some()
    }
    fn default_weight(&self) -> f32 {
        1.0
    }
    fn disabled_env_hint(&self) -> Option<&str> {
        if self.api_key.is_none() {
            Some("EXA_API_KEY")
        } else {
            None
        }
    }

    async fn search(&self, req: &SearchRequest) -> Result<Vec<SearchHit>, BackendError> {
        let api_key = self.api_key.as_ref().ok_or(BackendError::Disabled)?;

        // Build request body — Exa API (2026):
        //   type: "auto" | "neural" | "fast" (default is "auto" now)
        //   contents: { highlights: true } returns query-relevant chunks
        let num_results = req.per_backend_raw_cap().min(100);
        let mut body = json!({
            "query": req.query,
            "numResults": num_results,
            "type": "auto",
            "contents": {
                "highlights": true,
                "text": { "maxCharacters": 500 }
            }
        });

        if let Some(obj) = body.as_object_mut() {
            if let Some(cat) = &req.category {
                let cat_str = match cat {
                    Category::Company => "company",
                    Category::ResearchPaper => "research paper",
                    Category::News => "news",
                    Category::PersonalSite => "personal site",
                    Category::FinancialReport => "financial report",
                    Category::People => "people",
                    Category::Code => "github", // Exa uses "github" for code
                };
                obj.insert("category".into(), json!(cat_str));
            }
            if !req.include_domains.is_empty() {
                obj.insert("includeDomains".into(), json!(req.include_domains));
            }
            if !req.exclude_domains.is_empty() {
                obj.insert("excludeDomains".into(), json!(req.exclude_domains));
            }
        }

        let request = self
            .client
            .post("https://api.exa.ai/search")
            .header("x-api-key", api_key)
            .header("x-exa-integration", "temm1e")
            .header("Content-Type", "application/json")
            .json(&body);

        let body_str = fetch_bounded(&self.client, request).await?;
        let parsed: ApiResponse = serde_json::from_str(&body_str)
            .map_err(|e| BackendError::Parse(format!("exa json: {e}")))?;

        let total = parsed.results.len();
        let hits: Vec<SearchHit> = parsed
            .results
            .into_iter()
            .enumerate()
            .map(|(i, r)| {
                let title = r
                    .title
                    .unwrap_or_else(|| r.id.clone().unwrap_or_else(|| "(no title)".into()));
                let snippet = build_exa_snippet(&r.highlights, &r.summary, &r.text);
                let score = if total == 0 {
                    0.0
                } else {
                    1.0 - (i as f32 / total as f32)
                };
                SearchHit {
                    title: truncate_safe(&title, 200),
                    url: r.url,
                    snippet: truncate_safe(&snippet, 500),
                    source: BackendId::Exa,
                    source_name: "exa".into(),
                    published: r.published_date,
                    score,
                    signal: None,
                    also_in: vec![],
                }
            })
            .collect();

        Ok(hits)
    }
}

/// Build a readable snippet from Exa's content fields.
/// Prefers highlights (query-relevant chunks), then summary, then raw text.
fn build_exa_snippet(
    highlights: &Option<Vec<String>>,
    summary: &Option<String>,
    text: &Option<String>,
) -> String {
    if let Some(hs) = highlights {
        if !hs.is_empty() {
            return hs.join(" · ");
        }
    }
    if let Some(s) = summary {
        if !s.is_empty() {
            return s.clone();
        }
    }
    text.clone().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exa_disabled_when_no_key() {
        // Intentionally clear for this test
        std::env::remove_var("EXA_API_KEY");
        let b = ExaBackend::new();
        assert!(!b.enabled());
        assert_eq!(b.disabled_env_hint(), Some("EXA_API_KEY"));
    }

    #[test]
    fn parse_exa_response_with_highlights() {
        let json = r#"{
            "requestId": "abc",
            "results": [
                {
                    "id": "exa-1",
                    "title": "Example Result",
                    "url": "https://example.com/page",
                    "publishedDate": "2026-04-01",
                    "highlights": ["First relevant chunk", "Second relevant chunk"]
                }
            ]
        }"#;
        let parsed: ApiResponse = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.results.len(), 1);
        assert_eq!(parsed.results[0].url, "https://example.com/page");
        assert_eq!(
            parsed.results[0].highlights.as_ref().map(|h| h.len()),
            Some(2)
        );
    }

    #[test]
    fn build_exa_snippet_prefers_highlights() {
        let s = build_exa_snippet(
            &Some(vec!["chunk one".into(), "chunk two".into()]),
            &Some("summary text".into()),
            &Some("full text".into()),
        );
        assert!(s.contains("chunk one"));
        assert!(s.contains("chunk two"));
        assert!(!s.contains("summary"));
    }

    #[test]
    fn build_exa_snippet_falls_through_to_summary() {
        let s = build_exa_snippet(
            &None,
            &Some("summary text".into()),
            &Some("full text".into()),
        );
        assert_eq!(s, "summary text");
    }

    #[test]
    fn build_exa_snippet_falls_through_to_text() {
        let s = build_exa_snippet(&None, &None, &Some("just text".into()));
        assert_eq!(s, "just text");
    }

    #[test]
    fn build_exa_snippet_empty() {
        let s = build_exa_snippet(&None, &None, &None);
        assert_eq!(s, "");
    }
}
