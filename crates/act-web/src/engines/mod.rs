//! Search engine registry: multi-engine, parallel, merged via Reciprocal
//! Rank Fusion. Engines available without keys: DuckDuckGo, Bing. Optional:
//! Google (SERPAPI key), Brave (API key), SearXNG (self-hosted endpoint).

pub mod bing;
pub mod brave;
pub mod duckduckgo;
pub mod google;
pub mod searxng;

use std::collections::HashMap;
use std::sync::Arc;

use act_kernel::error::ActResult;
use act_kernel::SandboxContext;
use async_trait::async_trait;

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct MergedHit {
    pub rank: usize,
    pub title: String,
    pub url: String,
    pub snippet: String,
    pub engines: Vec<String>,
    pub score: f64,
}

#[async_trait]
pub trait SearchEngine: Send + Sync {
    fn name(&self) -> &'static str;
    /// Search one query; return up to `count` hits.
    async fn search(
        &self,
        ctx: &SandboxContext,
        query: &str,
        count: usize,
    ) -> ActResult<Vec<SearchHit>>;
}

/// Engines enabled by config and credentials.
pub fn available(ctx: &SandboxContext) -> Vec<Arc<dyn SearchEngine>> {
    let mut all: Vec<Arc<dyn SearchEngine>> =
        vec![Arc::new(duckduckgo::DuckDuckGo), Arc::new(bing::Bing)];
    if let Some(engine) = google::Google::from_env(ctx) {
        all.push(Arc::new(engine));
    }
    if let Some(engine) = brave::Brave::from_env(ctx) {
        all.push(Arc::new(engine));
    }
    if let Some(engine) = searxng::SearXng::from_config(ctx) {
        all.push(Arc::new(engine));
    }

    let enabled = &ctx.config.engines.enabled;
    if enabled.is_empty() {
        return all;
    }
    all.into_iter()
        .filter(|e| enabled.iter().any(|n| n == e.name()))
        .collect()
}

/// Merge hits from multiple engines with Reciprocal Rank Fusion.
pub fn merge(per_engine: &[(String, Vec<SearchHit>)], limit: usize) -> Vec<MergedHit> {
    // score = sum over engines of 1 / (60 + rank)
    let mut scores: HashMap<String, f64> = HashMap::new();
    // key -> (title, url, snippet, engines)
    let mut info: HashMap<String, (String, String, String, Vec<String>)> = HashMap::new();
    for (engine, hits) in per_engine {
        for (rank, hit) in hits.iter().enumerate() {
            let key = normalize_url(&hit.url);
            if key.is_empty() {
                continue;
            }
            let entry = info.entry(key.clone()).or_insert_with(|| {
                (
                    hit.title.clone(),
                    hit.url.clone(),
                    hit.snippet.clone(),
                    Vec::new(),
                )
            });
            if entry.3.iter().all(|e| e != engine) {
                entry.3.push(engine.clone());
            }
            if hit.snippet.len() > entry.2.len() {
                entry.2 = hit.snippet.clone();
            }
            if hit.url.starts_with("http") {
                entry.1 = hit.url.clone();
            }
            *scores.entry(key).or_insert(0.0) += 1.0 / (60.0 + rank as f64);
        }
    }
    let mut merged: Vec<(String, f64, (String, String, String, Vec<String>))> = scores
        .into_iter()
        .map(|(key, score)| {
            let info = info
                .remove(&key)
                .unwrap_or_else(|| (String::new(), key.clone(), String::new(), vec![]));
            (key, score, info)
        })
        .collect();
    merged.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    merged
        .into_iter()
        .take(limit)
        .enumerate()
        .map(
            |(i, (_, score, (title, url, snippet, engines)))| MergedHit {
                rank: i + 1,
                title,
                url,
                snippet,
                engines,
                score,
            },
        )
        .collect()
}

/// Normalize a URL for dedupe: scheme-insensitive host/path key
/// (http/https treated as the same page, www dropped, trailing slash dropped).
pub fn normalize_url(url: &str) -> String {
    let Ok(parsed) = url::Url::parse(url) else {
        return url.trim().to_lowercase();
    };
    if parsed.scheme() != "https" && parsed.scheme() != "http" {
        return url.trim().to_lowercase();
    }
    let mut host = parsed.host_str().unwrap_or("").to_lowercase();
    if let Some(stripped) = host.strip_prefix("www.") {
        host = stripped.to_string();
    }
    let port = match parsed.port() {
        Some(port) if port != 80 && port != 443 => format!(":{port}"),
        _ => String::new(),
    };
    let path = parsed.path().trim_end_matches('/');
    format!("{host}{port}{path}")
}

/// Tiny percent-decoder (query-escape aware: '+' means space).
pub fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or("");
                if let Ok(byte) = u8::from_str_radix(hex, 16) {
                    out.push(byte);
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_dedupes_and_ranks() {
        let ddg = (
            "duckduckgo".to_string(),
            vec![
                SearchHit {
                    title: "A".into(),
                    url: "https://Example.com/a/".into(),
                    snippet: "sa".into(),
                },
                SearchHit {
                    title: "B".into(),
                    url: "https://b.org/x".into(),
                    snippet: "sb".into(),
                },
            ],
        );
        let bing = (
            "bing".to_string(),
            vec![SearchHit {
                title: "A2".into(),
                url: "http://www.example.com/a".into(),
                snippet: "sa2".into(),
            }],
        );
        let merged = merge(&[ddg, bing], 10);
        assert_eq!(merged.len(), 2);
        assert_eq!(
            merged[0].engines.len(),
            2,
            "same page found by both engines ranks first"
        );
        assert!(merged[0].url.to_lowercase().contains("example.com"));
    }

    #[test]
    fn percent_decoding() {
        assert_eq!(percent_decode("a%20b+c"), "a b c");
        assert_eq!(percent_decode("%E4%B8%AD"), "中");
    }
}
