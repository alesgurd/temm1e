//! Self-hosted SearXNG backend.
//!
//! Endpoint: http://{user's SearXNG URL}/search?q=Q&format=json
//! Auth: none (self-hosted, under user's control).
//!
//! Reads the URL from:
//! 1. Env var `TEMM1E_SEARXNG_URL`
//! 2. Constructor `searxng_url` argument (from config)
//!
//! Falls through to `enabled() = false` when unset — the backend is
//! silently absent from the default mix until the user configures it.
//!
//! See `temm1e search install` (src/search_install.rs) for a one-click
//! Docker-based setup that writes the URL to config automatically.

use super::{fetch_bounded, make_client, truncate_safe, SearchBackend};
use crate::web_search::types::*;
use async_trait::async_trait;
use serde::Deserialize;

pub struct SearxngBackend {
    client: reqwest::Client,
    url: Option<String>,
}

impl SearxngBackend {
    pub fn new(url_from_config: Option<String>) -> Self {
        // Env var takes precedence over config
        let url = std::env::var("TEMM1E_SEARXNG_URL")
            .ok()
            .filter(|s| !s.is_empty())
            .or(url_from_config)
            .map(|s| s.trim_end_matches('/').to_string());
        Self {
            client: make_client(),
            url,
        }
    }
}

impl Default for SearxngBackend {
    fn default() -> Self {
        Self::new(None)
    }
}

#[derive(Debug, Deserialize)]
struct ApiResponse {
    #[serde(default)]
    results: Vec<SearxngResult>,
}

#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct SearxngResult {
    url: String,
    title: String,
    #[serde(default)]
    content: Option<String>,
    #[serde(default)]
    engine: Option<String>,
    #[serde(default)]
    score: Option<f32>,
    #[serde(default, rename = "publishedDate")]
    published_date: Option<String>,
}

#[async_trait]
impl SearchBackend for SearxngBackend {
    fn id(&self) -> BackendId {
        BackendId::SearXng
    }
    fn name(&self) -> &str {
        "searxng"
    }
    fn enabled(&self) -> bool {
        self.url.is_some()
    }
    fn default_weight(&self) -> f32 {
        1.0
    }
    fn disabled_env_hint(&self) -> Option<&str> {
        if self.url.is_none() {
            Some("TEMM1E_SEARXNG_URL or run `temm1e search install`")
        } else {
            None
        }
    }

    async fn search(&self, req: &SearchRequest) -> Result<Vec<SearchHit>, BackendError> {
        let base = self.url.as_ref().ok_or(BackendError::Disabled)?;
        let endpoint = format!("{base}/search");
        let request = self
            .client
            .get(&endpoint)
            .query(&[("q", req.query.as_str()), ("format", "json")]);

        let body = fetch_bounded(&self.client, request).await?;
        let parsed: ApiResponse = serde_json::from_str(&body)
            .map_err(|e| BackendError::Parse(format!("searxng json: {e}")))?;

        let total = parsed.results.len();
        let hits: Vec<SearchHit> = parsed
            .results
            .into_iter()
            .enumerate()
            .map(|(i, r)| {
                let snippet = r
                    .content
                    .as_deref()
                    .map(|c| truncate_safe(c, 200))
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
                    source: BackendId::SearXng,
                    source_name: "searxng".into(),
                    published: r.published_date,
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
    fn searxng_disabled_when_no_url() {
        // Clear any env var for this test
        std::env::remove_var("TEMM1E_SEARXNG_URL");
        let b = SearxngBackend::new(None);
        assert!(!b.enabled());
        assert!(b.disabled_env_hint().is_some());
    }

    #[test]
    fn searxng_enabled_from_config() {
        std::env::remove_var("TEMM1E_SEARXNG_URL");
        let b = SearxngBackend::new(Some("http://localhost:8888".into()));
        assert!(b.enabled());
        assert!(b.disabled_env_hint().is_none());
    }

    #[test]
    fn searxng_trailing_slash_stripped() {
        std::env::remove_var("TEMM1E_SEARXNG_URL");
        let b = SearxngBackend::new(Some("http://localhost:8888/".into()));
        assert_eq!(b.url.as_deref(), Some("http://localhost:8888"));
    }

    #[test]
    fn parse_searxng_response() {
        let json = r#"{
            "query": "test",
            "results": [
                {
                    "url": "https://example.com/page",
                    "title": "Example Page",
                    "content": "An example web page",
                    "engine": "google",
                    "score": 0.92,
                    "publishedDate": "2026-04-10"
                }
            ]
        }"#;
        let parsed: ApiResponse = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.results.len(), 1);
        assert_eq!(parsed.results[0].title, "Example Page");
        assert_eq!(parsed.results[0].score, Some(0.92));
    }

    #[test]
    fn parse_searxng_empty_results() {
        let json = r#"{"query": "test", "results": []}"#;
        let parsed: ApiResponse = serde_json::from_str(json).unwrap();
        assert!(parsed.results.is_empty());
    }
}
