//! SearXNG instance (self-hosted, configured via engines.searxng_url).

use act_kernel::error::{ActError, ActResult};
use act_kernel::SandboxContext;
use async_trait::async_trait;
use serde_json::Value;

use super::duckduckgo::urlencode;
use super::{SearchEngine, SearchHit};
use crate::http::{fetch, FetchOpts};

pub struct SearXng {
    base_url: String,
}

impl SearXng {
    pub fn from_config(ctx: &SandboxContext) -> Option<Self> {
        ctx.config
            .engines
            .searxng_url
            .as_ref()
            .filter(|u| !u.is_empty())
            .map(|base_url| Self {
                base_url: base_url.trim_end_matches('/').to_string(),
            })
    }
}

#[async_trait]
impl SearchEngine for SearXng {
    fn name(&self) -> &'static str {
        "searxng"
    }

    async fn search(
        &self,
        ctx: &SandboxContext,
        query: &str,
        count: usize,
    ) -> ActResult<Vec<SearchHit>> {
        let url = format!(
            "{}/search?q={}&format=json",
            self.base_url,
            urlencode(query)
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
            .map_err(|e| ActError::execution("searxng", format!("searxng response: {e}")))?;
        let Some(results) = body.get("results").and_then(|v| v.as_array()) else {
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
                    .get("content")
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
