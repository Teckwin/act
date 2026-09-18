//! Google via SERPAPI (requires SERPAPI_KEY or the configured env var).

use act_kernel::error::{ActError, ActResult};
use act_kernel::SandboxContext;
use async_trait::async_trait;
use serde_json::Value;

use super::duckduckgo::urlencode;
use super::{SearchEngine, SearchHit};
use crate::http::{fetch, FetchOpts};

pub struct Google {
    api_key: String,
}

impl Google {
    pub fn from_env(ctx: &SandboxContext) -> Option<Self> {
        let var = if ctx.config.engines.google_serpapi_key_env.is_empty() {
            "SERPAPI_KEY".to_string()
        } else {
            ctx.config.engines.google_serpapi_key_env.clone()
        };
        std::env::var(&var)
            .ok()
            .filter(|k| !k.is_empty())
            .map(|api_key| Self { api_key })
    }
}

#[async_trait]
impl SearchEngine for Google {
    fn name(&self) -> &'static str {
        "google"
    }

    async fn search(
        &self,
        ctx: &SandboxContext,
        query: &str,
        count: usize,
    ) -> ActResult<Vec<SearchHit>> {
        let url = format!(
            "https://serpapi.com/search.json?engine=google&q={}&num={}&api_key={}",
            urlencode(query),
            count,
            urlencode(&self.api_key)
        );
        let outcome = fetch(
            ctx,
            &url,
            &FetchOpts {
                max_bytes: ctx.config.limits.fetch_max_bytes,
                timeout_ms: ctx.config.limits.web_timeout_ms,
                headers: vec![],
            },
        )
        .await?;
        let body: Value = serde_json::from_slice(&outcome.body)
            .map_err(|e| ActError::execution("google", format!("serpapi response: {e}")))?;
        let Some(organic) = body.get("organic_results").and_then(|v| v.as_array()) else {
            return Ok(Vec::new());
        };
        Ok(organic
            .iter()
            .filter_map(|item| {
                let title = item.get("title")?.as_str()?.trim().to_string();
                let url = item.get("link")?.as_str()?.trim().to_string();
                if title.is_empty() || url.is_empty() {
                    return None;
                }
                let snippet = item
                    .get("snippet")
                    .and_then(|s| s.as_str())
                    .unwrap_or_default()
                    .to_string();
                Some(SearchHit {
                    title,
                    url,
                    snippet,
                })
            })
            .take(count)
            .collect())
    }
}
