//! Tavily search API (paid, opt-in).
//!
//! Endpoint: https://api.tavily.com/search (POST)
//! Auth: `Authorization: Bearer tvly-$TAVILY_API_KEY` header.
//! (The legacy body-based `api_key` is deprecated as of 2026 — we use the header.)
//!
//! Auto-registered but disabled unless `TAVILY_API_KEY` env var is set.
//! Free tier: 1000 credits/month. `basic` search depth = 1 credit,
//! `advanced` = 2 credits.

use super::{fetch_bounded, make_client, truncate_safe, SearchBackend};
use crate::web_search::types::*;
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

pub struct TavilyBackend {
    client: reqwest::Client,
    api_key: Option<String>,
}

impl TavilyBackend {
    pub fn new() -> Self {
        let api_key = std::env::var("TAVILY_API_KEY")
            .ok()
            .filter(|s| !s.is_empty());
        Self {
            client: make_client(),
            api_key,
        }
    }
}

impl Default for TavilyBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Deserialize)]
struct ApiResponse {
    #[serde(default)]
    results: Vec<TavilyResult>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct TavilyResult {
    title: String,
    url: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    score: Option<f32>,
    #[serde(default)]
    raw_content: Option<String>,
}

#[async_trait]
impl SearchBackend for TavilyBackend {
    fn id(&self) -> BackendId {
        BackendId::Tavily
    }
    fn name(&self) -> &str {
        "tavily"
    }
    fn enabled(&self) -> bool {
        self.api_key.is_some()
    }
    fn default_weight(&self) -> f32 {
        0.95
    }
    fn disabled_env_hint(&self) -> Option<&str> {
        if self.api_key.is_none() {
            Some("TAVILY_API_KEY")
        } else {
            None
        }
    }

    async fn search(&self, req: &SearchRequest) -> Result<Vec<SearchHit>, BackendError> {
        let api_key = self.api_key.as_ref().ok_or(BackendError::Disabled)?;

        // Tavily accepts key as "tvly-..." or raw. Pass as-is.
        let bearer = if api_key.starts_with("tvly-") {
            api_key.clone()
        } else {
            format!("tvly-{api_key}")
        };

        let mut body = json!({
            "query": req.query,
            "search_depth": "basic",
            "max_results": req.per_backend_raw_cap().min(20),
        });

        if let Some(obj) = body.as_object_mut() {
            // time_range → Tavily accepts day|week|month|year or d|w|m|y
            let tr = match req.time_range {
                TimeRange::Day => Some("day"),
                TimeRange::Week => Some("week"),
                TimeRange::Month => Some("month"),
                TimeRange::Year => Some("year"),
                TimeRange::All => None,
            };
            if let Some(t) = tr {
                obj.insert("time_range".into(), json!(t));
            }
            if !req.include_domains.is_empty() {
                obj.insert("include_domains".into(), json!(req.include_domains));
            }
            if !req.exclude_domains.is_empty() {
                obj.insert("exclude_domains".into(), json!(req.exclude_domains));
            }
        }

        let request = self
            .client
            .post("https://api.tavily.com/search")
            .header("Authorization", format!("Bearer {bearer}"))
            .header("Content-Type", "application/json")
            .json(&body);

        let body_str = fetch_bounded(&self.client, request).await?;
        let parsed: ApiResponse = serde_json::from_str(&body_str)
            .map_err(|e| BackendError::Parse(format!("tavily json: {e}")))?;

        let total = parsed.results.len();
        let hits: Vec<SearchHit> = parsed
            .results
            .into_iter()
            .enumerate()
            .map(|(i, r)| {
                let snippet = r
                    .content
                    .as_deref()
                    .map(|c| truncate_safe(c, 300))
                    .unwrap_or_default();
                let score = r.score.unwrap_or_else(|| {
                    if total == 0 {
                        0.0
                    } else {
                        1.0 - (i as f32 / total as f32)
                    }
                });
                SearchHit {
                    title: r.title,
                    url: r.url,
                    snippet,
                    source: BackendId::Tavily,
                    source_name: "tavily".into(),
                    published: None, // Tavily does not return a published_date field
                    score: score.clamp(0.0, 1.0),
                    signal: None,
                    also_in: vec![],
                }
            })
            .collect();

        Ok(hits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tavily_disabled_when_no_key() {
        std::env::remove_var("TAVILY_API_KEY");
        let b = TavilyBackend::new();
        assert!(!b.enabled());
        assert_eq!(b.disabled_env_hint(), Some("TAVILY_API_KEY"));
    }

    #[test]
    fn parse_tavily_response() {
        let json = r#"{
            "query": "test",
            "answer": "An answer summary",
            "results": [
                {
                    "title": "Example Result",
                    "url": "https://example.com/page",
                    "content": "The relevant content from the page",
                    "score": 0.87
                }
            ]
        }"#;
        let parsed: ApiResponse = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.results.len(), 1);
        assert_eq!(parsed.results[0].title, "Example Result");
        assert_eq!(parsed.results[0].score, Some(0.87));
    }

    #[test]
    fn parse_tavily_empty_results() {
        let json = r#"{"query": "test", "results": []}"#;
        let parsed: ApiResponse = serde_json::from_str(json).unwrap();
        assert!(parsed.results.is_empty());
    }
}
