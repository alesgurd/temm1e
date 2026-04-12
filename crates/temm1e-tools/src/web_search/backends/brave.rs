//! Brave Search API (paid, opt-in).
//!
//! Endpoint: https://api.search.brave.com/res/v1/web/search (GET)
//! Auth: `X-Subscription-Token: $BRAVE_API_KEY` header.
//! Free tier was removed Dec 2025 — set up a paid plan at api-dashboard.search.brave.com.
//!
//! Auto-registered but disabled unless `BRAVE_API_KEY` env var is set.

use super::{fetch_bounded, make_client, truncate_safe, SearchBackend};
use crate::web_search::types::*;
use async_trait::async_trait;
use serde::Deserialize;

pub struct BraveBackend {
    client: reqwest::Client,
    api_key: Option<String>,
}

impl BraveBackend {
    pub fn new() -> Self {
        let api_key = std::env::var("BRAVE_API_KEY")
            .ok()
            .filter(|s| !s.is_empty());
        Self {
            client: make_client(),
            api_key,
        }
    }
}

impl Default for BraveBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Deserialize)]
struct ApiResponse {
    #[serde(default)]
    web: Option<WebSection>,
}

#[derive(Debug, Deserialize)]
struct WebSection {
    #[serde(default)]
    results: Vec<BraveResult>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct BraveResult {
    title: String,
    url: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    age: Option<String>,
    #[serde(default)]
    page_age: Option<String>,
    #[serde(default)]
    language: Option<String>,
}

#[async_trait]
impl SearchBackend for BraveBackend {
    fn id(&self) -> BackendId {
        BackendId::Brave
    }
    fn name(&self) -> &str {
        "brave"
    }
    fn enabled(&self) -> bool {
        self.api_key.is_some()
    }
    fn default_weight(&self) -> f32 {
        0.95
    }
    fn disabled_env_hint(&self) -> Option<&str> {
        if self.api_key.is_none() {
            Some("BRAVE_API_KEY")
        } else {
            None
        }
    }

    async fn search(&self, req: &SearchRequest) -> Result<Vec<SearchHit>, BackendError> {
        let api_key = self.api_key.as_ref().ok_or(BackendError::Disabled)?;

        // count max is 20 per Brave API
        let count = req.per_backend_raw_cap().min(20);
        let mut params: Vec<(&str, String)> =
            vec![("q", req.query.clone()), ("count", count.to_string())];
        if let Some(lang) = &req.language {
            params.push(("search_lang", lang.clone()));
        }
        if let Some(region) = &req.region {
            params.push(("country", region.to_uppercase()));
        }
        // time_range → freshness: pd|pw|pm|py
        let freshness = match req.time_range {
            TimeRange::Day => Some("pd"),
            TimeRange::Week => Some("pw"),
            TimeRange::Month => Some("pm"),
            TimeRange::Year => Some("py"),
            TimeRange::All => None,
        };
        if let Some(f) = freshness {
            params.push(("freshness", f.to_string()));
        }

        let request = self
            .client
            .get("https://api.search.brave.com/res/v1/web/search")
            .header("X-Subscription-Token", api_key)
            .header("Accept", "application/json")
            .header("Accept-Encoding", "gzip")
            .query(&params);

        let body = fetch_bounded(&self.client, request).await?;
        let parsed: ApiResponse = serde_json::from_str(&body)
            .map_err(|e| BackendError::Parse(format!("brave json: {e}")))?;

        let results = parsed.web.map(|w| w.results).unwrap_or_default();
        let total = results.len();

        let hits: Vec<SearchHit> = results
            .into_iter()
            .enumerate()
            .map(|(i, r)| {
                let snippet = r
                    .description
                    .as_deref()
                    .map(|d| truncate_safe(d, 300))
                    .unwrap_or_default();
                let score = if total == 0 {
                    0.0
                } else {
                    1.0 - (i as f32 / total as f32)
                };
                SearchHit {
                    title: r.title,
                    url: r.url,
                    snippet,
                    source: BackendId::Brave,
                    source_name: "brave".into(),
                    published: r.page_age.or(r.age),
                    score,
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
    fn brave_disabled_when_no_key() {
        std::env::remove_var("BRAVE_API_KEY");
        let b = BraveBackend::new();
        assert!(!b.enabled());
        assert_eq!(b.disabled_env_hint(), Some("BRAVE_API_KEY"));
    }

    #[test]
    fn parse_brave_response() {
        let json = r#"{
            "type": "search",
            "web": {
                "type": "search",
                "results": [
                    {
                        "title": "Example Page",
                        "url": "https://example.com/page",
                        "description": "An example description",
                        "age": "2 days ago",
                        "language": "en"
                    }
                ]
            }
        }"#;
        let parsed: ApiResponse = serde_json::from_str(json).unwrap();
        let results = parsed.web.unwrap().results;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].title, "Example Page");
        assert_eq!(results[0].age.as_deref(), Some("2 days ago"));
    }

    #[test]
    fn parse_brave_empty_web_section() {
        let json = r#"{"type": "search", "query": {"original": "test"}}"#;
        let parsed: ApiResponse = serde_json::from_str(json).unwrap();
        assert!(parsed.web.is_none());
    }

    #[test]
    fn parse_brave_web_empty_results() {
        let json = r#"{"type": "search", "web": {"results": []}}"#;
        let parsed: ApiResponse = serde_json::from_str(json).unwrap();
        assert!(parsed.web.unwrap().results.is_empty());
    }
}
