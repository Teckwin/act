//! Brave Search API (requires BRAVE_API_KEY or the configured env var).

use act_kernel::error::{ActError, ActResult};
use act_kernel::SandboxContext;
use async_trait::async_trait;
use serde_json::Value;

use super::duckduckgo::urlencode;
use super::{SearchEngine, SearchHit};
use crate::http::{fetch, FetchOpts};

pub struct Brave {
    api_key: String,
}

impl Brave {
    pub fn from_env(ctx: &SandboxContext) -> Option<Self> {
        let var = if ctx.config.engines.brave_api_key_env.is_empty() {
            "BRAVE_API_KEY".to_string()
        } else {
            ctx.config.engines.brave_api_key_env.clone()
        };
        std::env::var(&var)
            .ok()
            .filter(|k| !k.is_empty())
            .map(|api_key| Self { api_key })
    }
}

#[async_trait]
impl SearchEngine for Brave {
    fn name(&self) -> &'static str {
        "brave"
    }

    async fn search(
        &self,
        ctx: &SandboxContext,
        query: &str,
        count: usize,
    ) -> ActResult<Vec<SearchHit>> {
        let url = format!(
            "https://api.search.brave.com/res/v1/web/search?q={}&count={}",
            urlencode(query),
            count
        );
        let outcome = fetch(
            ctx,
            &url,
            &FetchOpts {
                max_bytes: ctx.config.limits.fetch_max_bytes,
                timeout_ms: ctx.config.limits.web_timeout_ms,
                headers: vec![("X-Subscription-Token".to_string(), self.api_key.clone())],
            },
        )
        .await;
        let outcome = match outcome {
            Ok(o) => o,
            Err(err) => {
                return Err(ActError::execution(
                    "brave",
                    format!("brave search failed: {err}"),
                ));
            }
        };
        let body: Value = serde_json::from_slice(&outcome.body)
            .map_err(|e| ActError::execution("brave", format!("brave response: {e}")))?;
        let Some(results) = body.pointer("/web/results").and_then(|v| v.as_array()) else {
            return Ok(Vec::new());
        };
        Ok(results
            .iter()
            .filter_map(|item| {
                let title = item.get("title")?.as_str()?.trim().to_string();
                let url = item.get("url")?.as_str()?.trim().to_string();
                if title.is_empty() || url.is_empty() {
                    return None;
                }
                let snippet = item
                    .get("description")
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
