//! DuckDuckGo via the HTML endpoint.
//!
//! Endpoint: https://html.duckduckgo.com/html/?q=QUERY (GET only)
//! Auth: none. Uses realistic Chrome User-Agent + governor (10/min) to stay
//! under the bot-detection threshold.
//!
//! We parse HTML with regex rather than a DOM parser — DDG's HTML endpoint
//! has a stable, simple structure (`.result__a`, `.result__snippet`, etc.)
//! and a regex-based extractor is ~40 lines and has zero new deps.
//!
//! Live-verified 2026-04-12: GET with Chrome UA returns 200 OK with 10
//! parseable organic results. See docs/web_search/RESEARCH.md §2.

use super::{fetch_bounded, truncate_safe, SearchBackend};
use crate::web_search::types::*;
use async_trait::async_trait;
use std::sync::OnceLock;

/// Realistic Chrome UA — matches the one used by BrowserTool for stealth parity.
const DDG_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
    (KHTML, like Gecko) Chrome/134.0.0.0 Safari/537.36";

pub struct DuckDuckGoBackend {
    client: reqwest::Client,
}

impl DuckDuckGoBackend {
    pub fn new() -> Self {
        // Build a client with the Chrome UA specifically — don't inherit
        // the default Tem/ UA from make_client() since DDG blocks known bots.
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(DEFAULT_BACKEND_TIMEOUT_SECS))
            .user_agent(DDG_USER_AGENT)
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { client }
    }
}

impl Default for DuckDuckGoBackend {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl SearchBackend for DuckDuckGoBackend {
    fn id(&self) -> BackendId {
        BackendId::DuckDuckGo
    }
    fn name(&self) -> &str {
        "duckduckgo"
    }
    fn enabled(&self) -> bool {
        true
    }
    fn default_weight(&self) -> f32 {
        0.95
    }

    async fn search(&self, req: &SearchRequest) -> Result<Vec<SearchHit>, BackendError> {
        let endpoint = "https://html.duckduckgo.com/html/";
        let request = self
            .client
            .get(endpoint)
            .header("Accept", "text/html,application/xhtml+xml")
            .header("Accept-Language", "en-US,en;q=0.9")
            .header("Referer", "https://duckduckgo.com/")
            .query(&[("q", req.query.as_str())]);

        let html = fetch_bounded(&self.client, request).await?;

        // DDG's html endpoint responds 200 with an empty body + a refresh
        // meta tag when rate-limited. Detect and surface.
        if html.len() < 500 {
            return Err(BackendError::RateLimited {
                retry_after_ms: 60_000,
            });
        }
        // If the response is the anomaly detection page, bail gracefully.
        if html.contains("anomaly") || html.contains("unusual traffic") || html.contains("CAPTCHA")
        {
            return Err(BackendError::RateLimited {
                retry_after_ms: 60_000,
            });
        }

        let cap = req.per_backend_raw_cap();
        let results = parse_ddg_html(&html, cap);
        let total = results.len();

        let hits: Vec<SearchHit> = results
            .into_iter()
            .enumerate()
            .map(|(i, r)| {
                let score = if total == 0 {
                    0.0
                } else {
                    1.0 - (i as f32 / total as f32)
                };
                SearchHit {
                    title: truncate_safe(&r.title, 200),
                    url: r.url,
                    snippet: truncate_safe(&r.snippet, 300),
                    source: BackendId::DuckDuckGo,
                    source_name: "duckduckgo".into(),
                    published: None,
                    score,
                    signal: None,
                    also_in: vec![],
                }
            })
            .collect();

        Ok(hits)
    }
}

#[derive(Debug, Clone)]
struct DdgResult {
    title: String,
    url: String,
    snippet: String,
}

/// Extract DDG results from the HTML response via regex.
///
/// DDG's structure is stable:
/// ```html
/// <a class="result__a" href="URL">TITLE</a>
/// ...
/// <a class="result__snippet" ...>SNIPPET</a>
/// ```
/// We match anchor tags with each class in order and zip them. DDG wraps
/// URLs through `/l/?uddg=ENCODED` — we unwrap that.
fn parse_ddg_html(html: &str, max: usize) -> Vec<DdgResult> {
    static TITLE_RE: OnceLock<regex::Regex> = OnceLock::new();
    static SNIPPET_RE: OnceLock<regex::Regex> = OnceLock::new();

    let title_re = TITLE_RE.get_or_init(|| {
        // Match <a class="result__a" href="URL">TITLE</a>
        // Allow additional attributes between class and href or after href.
        regex::Regex::new(r#"(?is)<a[^>]*class="result__a"[^>]*href="([^"]+)"[^>]*>(.*?)</a>"#)
            .expect("static regex")
    });
    let snippet_re = SNIPPET_RE.get_or_init(|| {
        // Match <a class="result__snippet" ...>SNIPPET</a>
        // OR <div class="result__snippet" ...>SNIPPET</div>
        regex::Regex::new(r#"(?is)<(?:a|div)[^>]*class="result__snippet"[^>]*>(.*?)</(?:a|div)>"#)
            .expect("static regex")
    });

    let titles: Vec<(String, String)> = title_re
        .captures_iter(html)
        .take(max)
        .filter_map(|c| {
            let url_raw = c.get(1)?.as_str().to_string();
            let title_raw = c.get(2)?.as_str().to_string();
            let url = unwrap_ddg_redirect(&url_raw);
            let title = strip_html_text(&title_raw);
            if title.is_empty() || url.is_empty() {
                None
            } else {
                Some((url, title))
            }
        })
        .collect();

    let snippets: Vec<String> = snippet_re
        .captures_iter(html)
        .take(max)
        .filter_map(|c| c.get(1).map(|m| strip_html_text(m.as_str())))
        .collect();

    titles
        .into_iter()
        .enumerate()
        .map(|(i, (url, title))| DdgResult {
            title,
            url,
            snippet: snippets.get(i).cloned().unwrap_or_default(),
        })
        .collect()
}

/// DDG wraps organic URLs in `/l/?uddg=<url-encoded-target>&rut=...`.
/// Extract the real target. If the input doesn't match the wrapper shape,
/// return it unchanged.
fn unwrap_ddg_redirect(raw: &str) -> String {
    // Handle //duckduckgo.com/l/?uddg= or /l/?uddg= patterns
    let marker = "uddg=";
    if let Some(idx) = raw.find(marker) {
        let after = &raw[idx + marker.len()..];
        let end = after.find('&').unwrap_or(after.len());
        let encoded = &after[..end];
        return percent_decode(encoded);
    }
    raw.to_string()
}

/// Minimal percent-decoder for DDG redirect unwrapping.
/// Only handles the subset of encodings DDG emits (mostly %XX).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// Strip HTML tags and decode common entities from DDG result text.
fn strip_html_text(s: &str) -> String {
    use std::sync::OnceLock;
    static TAG_RE: OnceLock<regex::Regex> = OnceLock::new();
    let tag_re = TAG_RE.get_or_init(|| regex::Regex::new(r"<[^>]+>").expect("static"));
    let stripped = tag_re.replace_all(s, "");
    stripped
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ddg_backend_construction() {
        let b = DuckDuckGoBackend::new();
        assert_eq!(b.name(), "duckduckgo");
        assert!(b.enabled());
        assert!(matches!(b.id(), BackendId::DuckDuckGo));
    }

    #[test]
    fn unwrap_ddg_redirect_simple() {
        let raw = "//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fpage&rut=abc";
        let unwrapped = unwrap_ddg_redirect(raw);
        assert_eq!(unwrapped, "https://example.com/page");
    }

    #[test]
    fn unwrap_ddg_redirect_no_wrapper() {
        let raw = "https://example.com/direct";
        assert_eq!(unwrap_ddg_redirect(raw), "https://example.com/direct");
    }

    #[test]
    fn unwrap_ddg_redirect_no_trailing_params() {
        let raw = "/l/?uddg=https%3A%2F%2Fexample.com%2Fpage";
        assert_eq!(unwrap_ddg_redirect(raw), "https://example.com/page");
    }

    #[test]
    fn percent_decode_basic() {
        assert_eq!(percent_decode("hello%20world"), "hello world");
        assert_eq!(percent_decode("%3A%2F%2F"), "://");
        assert_eq!(percent_decode("no-encoding"), "no-encoding");
    }

    #[test]
    fn percent_decode_utf8() {
        // e with dot below (ẹ) = U+1EB9 = bytes E1 BA B9
        // percent-encoded as %E1%BA%B9
        assert_eq!(percent_decode("%E1%BA%B9"), "ẹ");
    }

    #[test]
    fn strip_html_text_basic() {
        assert_eq!(strip_html_text("Hello <b>world</b>"), "Hello world");
        assert_eq!(strip_html_text("Tom &amp; Jerry"), "Tom & Jerry");
        assert_eq!(strip_html_text("&lt;Vec&gt;"), "<Vec>");
        assert_eq!(strip_html_text("   spaces   "), "spaces");
    }

    #[test]
    fn parse_ddg_html_finds_results() {
        let html = r##"
            <html><body>
            <div class="result">
                <h2 class="result__title">
                    <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fone&amp;rut=abc">
                        Example One
                    </a>
                </h2>
                <a class="result__snippet" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fone">
                    This is the first example snippet.
                </a>
            </div>
            <div class="result">
                <h2 class="result__title">
                    <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Ftwo&amp;rut=def">
                        Example Two
                    </a>
                </h2>
                <a class="result__snippet" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Ftwo">
                    The second example with more text.
                </a>
            </div>
            </body></html>
        "##;
        let results = parse_ddg_html(html, 10);
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].title, "Example One");
        assert_eq!(results[0].url, "https://example.com/one");
        assert!(results[0].snippet.contains("first example"));
        assert_eq!(results[1].title, "Example Two");
        assert_eq!(results[1].url, "https://example.com/two");
    }

    #[test]
    fn parse_ddg_html_respects_max() {
        let mut html = String::from("<html><body>");
        for i in 0..20 {
            html.push_str(&format!(
                r##"<div class="result"><a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2F{i}">Result {i}</a><a class="result__snippet">Snippet {i}</a></div>"##,
            ));
        }
        html.push_str("</body></html>");
        let results = parse_ddg_html(&html, 5);
        assert_eq!(results.len(), 5);
    }

    #[test]
    fn parse_ddg_html_empty_returns_empty() {
        let results = parse_ddg_html("<html><body>no results</body></html>", 10);
        assert!(results.is_empty());
    }

    #[test]
    fn parse_ddg_html_handles_snippet_div() {
        let html = r##"
            <a class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fx.com">X</a>
            <div class="result__snippet">From a div not an anchor</div>
        "##;
        let results = parse_ddg_html(html, 10);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].snippet, "From a div not an anchor");
    }
}
