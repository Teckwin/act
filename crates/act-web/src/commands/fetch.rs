//! Web_Fetch: parallel URL fetching with HTML-to-Markdown extraction.

use std::sync::Arc;

use act_kernel::error::{ActError, ActResult};
use act_kernel::{Capability, CommandDef, CommandHandler, SandboxContext};
use async_trait::async_trait;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::json;

use crate::http::{fetch, FetchOpts};
use crate::markdown;

#[derive(Deserialize)]
struct Params {
    urls: Vec<String>,
    #[serde(default)]
    format: Option<String>,
    #[serde(default)]
    max_bytes: Option<u64>,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

pub struct WebFetch;

#[async_trait]
impl CommandHandler for WebFetch {
    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: &SandboxContext,
    ) -> ActResult<serde_json::Value> {
        let params: Params = serde_json::from_value(params)
            .map_err(|e| ActError::invalid_params("Web_Fetch", e.to_string()))?;
        if params.urls.is_empty() {
            return Err(ActError::invalid_params(
                "Web_Fetch",
                "'urls' must not be empty",
            ));
        }
        let limits = &ctx.config.limits;
        let opts = FetchOpts {
            max_bytes: params.max_bytes.unwrap_or(limits.fetch_max_bytes),
            timeout_ms: params.timeout_ms.unwrap_or(limits.web_timeout_ms),
            headers: vec![],
        };
        let format = params
            .format
            .as_deref()
            .unwrap_or("markdown")
            .to_lowercase();

        let concurrency = limits.web_concurrency;
        let urls: Vec<String> = params.urls.clone();
        let results: Vec<serde_json::Value> = futures::stream::iter(urls)
            .map(|url| {
                let opts = opts.clone();
                let format = format.clone();
                async move {
                    match fetch_one(ctx, &url, &opts, &format).await {
                        Ok(item) => item,
                        Err(err) => json!({
                            "url": url,
                            "ok": false,
                            "error": err.to_string(),
                            "code": err.code(),
                        }),
                    }
                }
            })
            .buffer_unordered(concurrency)
            .collect()
            .await;

        let succeeded = results
            .iter()
            .filter(|r| r.get("ok").and_then(|o| o.as_bool()).unwrap_or(false))
            .count();
        Ok(json!({
            "ok": succeeded > 0,
            "command": "Web_Fetch",
            "results": results,
            "summary": { "succeeded": succeeded, "failed": results.len() - succeeded },
        }))
    }
}

async fn fetch_one(
    ctx: &SandboxContext,
    url: &str,
    opts: &FetchOpts,
    format: &str,
) -> ActResult<serde_json::Value> {
    let outcome = fetch(ctx, url, opts).await?;
    let is_html = outcome.content_type.contains("text/html")
        || outcome.content_type.contains("application/xhtml");
    let charset = outcome
        .content_type
        .split(';')
        .find_map(|part| {
            part.trim()
                .strip_prefix("charset=")
                .map(|s| s.to_lowercase())
        })
        .unwrap_or_default();

    let decoded: String = if charset.is_empty() || charset == "utf-8" || charset == "utf8" {
        String::from_utf8_lossy(&outcome.body).into_owned()
    } else if let Some(enc) = encoding_rs::Encoding::for_label(charset.as_bytes()) {
        enc.decode(&outcome.body).0.into_owned()
    } else {
        String::from_utf8_lossy(&outcome.body).into_owned()
    };

    let content = match format {
        "html" => decoded,
        "json" => {
            let value: serde_json::Value = serde_json::from_str(&decoded).map_err(|e| {
                ActError::execution("Web_Fetch", format!("body is not valid JSON: {e}"))
            })?;
            serde_json::to_string_pretty(&value)
                .map_err(|e| ActError::Other(format!("json serialize: {e}")))?
        }
        "text" => {
            if is_html {
                markdown::to_text(&decoded)
            } else {
                decoded
            }
        }
        _ => {
            // markdown (default)
            if is_html {
                markdown::to_markdown(&decoded)?
            } else {
                decoded
            }
        }
    };

    Ok(json!({
        "url": url,
        "ok": true,
        "final_url": outcome.final_url,
        "status": outcome.status,
        "content_type": outcome.content_type,
        "bytes": outcome.body.len(),
        "truncated": outcome.truncated,
        "redirects": outcome.redirects,
        "content": content,
    }))
}

pub fn definition() -> ActResult<CommandDef> {
    CommandDef::new(
        "Web_Fetch",
        "Fetch URLs in parallel and convert HTML to clean Markdown (default), text, JSON or raw HTML. Every URL and redirect hop is policy-checked (SSRF-safe).",
        Capability::Net,
        json!({
            "type": "object",
            "properties": {
                "urls": { "type": "array", "items": { "type": "string" } },
                "format": { "type": "string", "enum": ["markdown", "text", "json", "html"], "default": "markdown" },
                "max_bytes": { "type": "integer" },
                "timeout_ms": { "type": "integer" }
            },
            "required": ["urls"]
        }),
        vec![],
        vec!["/urls/*".into()],
        Arc::new(WebFetch),
    )
}
