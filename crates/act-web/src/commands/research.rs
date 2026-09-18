//! Web_Research: search -> rank -> parallel fetch -> cited markdown report.
//! Purely heuristic (no LLM dependency): extractive summaries per source and
//! for the combined overview.

use std::sync::Arc;

use act_kernel::error::{ActError, ActResult};
use act_kernel::{
    builder::{CommandBuilder, Param},
    summarize, Capability, CommandDef, CommandHandler, SandboxContext,
};
use async_trait::async_trait;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::json;

use crate::commands::search::run_search;
use crate::engines::{self, SearchEngine};
use crate::http::{fetch, FetchOpts};
use crate::markdown;

#[derive(Deserialize)]
struct Params {
    topic: String,
    #[serde(default)]
    max_sources: Option<usize>,
    #[serde(default)]
    max_results: Option<usize>,
    #[serde(default)]
    engines: Option<Vec<String>>,
}

pub struct WebResearch;

#[async_trait]
impl CommandHandler for WebResearch {
    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: &SandboxContext,
    ) -> ActResult<serde_json::Value> {
        let params: Params = serde_json::from_value(params)
            .map_err(|e| ActError::invalid_params("Web_Research", e.to_string()))?;
        if params.topic.trim().is_empty() {
            return Err(ActError::invalid_params(
                "Web_Research",
                "'topic' must not be empty",
            ));
        }
        let max_sources = params.max_sources.unwrap_or(8).clamp(1, 20);
        let count = params.max_results.unwrap_or(15);

        let engine_list = engines::available(ctx);
        let selected: Vec<Arc<dyn SearchEngine>> = match &params.engines {
            Some(requested) => engine_list
                .into_iter()
                .filter(|e| requested.iter().any(|r| r == e.name()))
                .collect(),
            None => engine_list,
        };
        if selected.is_empty() {
            return Err(ActError::invalid_params(
                "Web_Research",
                "no search engines available",
            ));
        }
        run_research(ctx, &params.topic, &selected, max_sources, count).await
    }
}

pub async fn run_research(
    ctx: &SandboxContext,
    topic: &str,
    engine_list: &[Arc<dyn SearchEngine>],
    max_sources: usize,
    count: usize,
) -> ActResult<serde_json::Value> {
    // 1. Multi-engine search.
    let query = topic.trim().to_string();
    let search_result = run_search(ctx, &[query.clone()], engine_list, count).await;
    let results = search_result["queries"][0]["results"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if results.is_empty() {
        return Err(ActError::execution(
            "Web_Research",
            "no search results for the topic",
        ));
    }

    // 2. Pick top distinct sources.
    let mut picked: Vec<(String, String)> = Vec::new(); // (url, title)
    for item in &results {
        if picked.len() >= max_sources {
            break;
        }
        let url = item["url"].as_str().unwrap_or_default().to_string();
        let title = item["title"].as_str().unwrap_or_default().to_string();
        if url.is_empty() {
            continue;
        }
        picked.push((url, title));
    }

    // 3. Fetch sources in parallel (bounded).
    let concurrency = ctx.config.limits.web_concurrency;
    let opts = FetchOpts {
        max_bytes: ctx.config.limits.fetch_max_bytes,
        timeout_ms: ctx.config.limits.web_timeout_ms,
        headers: vec![],
    };
    let jobs: Vec<(usize, String, String)> = picked
        .iter()
        .enumerate()
        .map(|(idx, (url, title))| (idx, url.clone(), title.clone()))
        .collect();
    let fetched: Vec<(usize, Result<(String, String, usize), ActError>)> =
        futures::stream::iter(jobs)
            .map(|(idx, url, _title)| {
                let opts = opts.clone();
                async move {
                    let result = fetch(ctx, &url, &opts).await.and_then(|outcome| {
                        let text = String::from_utf8_lossy(&outcome.body).into_owned();
                        let md = if outcome.content_type.contains("html") {
                            markdown::to_markdown(&text).unwrap_or(text)
                        } else {
                            text
                        };
                        Ok((md, outcome.final_url.clone(), outcome.body.len()))
                    });
                    (idx, result)
                }
            })
            .buffer_unordered(concurrency)
            .collect()
            .await;

    // 4. Build per-source summaries.
    let mut sources: Vec<serde_json::Value> = Vec::new();
    let mut failures: Vec<serde_json::Value> = Vec::new();
    let mut content_by_index: Vec<Option<String>> = vec![None; picked.len()];
    for (idx, result) in fetched {
        match result {
            Ok((md, final_url, bytes)) => {
                content_by_index[idx] = Some(md.clone());
                let summary = summarize::extractive(&md, 400);
                sources.push(json!({
                    "n": sources.len() + 1,
                    "title": picked[idx].1,
                    "url": picked[idx].0,
                    "final_url": final_url,
                    "bytes": bytes,
                    "summary": summary,
                }));
            }
            Err(err) => {
                failures.push(json!({
                    "url": picked[idx].0,
                    "error": err.to_string(),
                }));
            }
        }
    }

    if sources.is_empty() {
        return Err(ActError::execution(
            "Web_Research",
            "all source fetches failed; see warnings in audit log",
        ));
    }

    // 5. Combined overview.
    let combined: String = content_by_index
        .iter()
        .flatten()
        .map(|md| {
            let head: String = md.chars().take(4000).collect();
            format!("{}\n", head)
        })
        .collect();
    let overview = summarize::extractive(&combined, 800);

    // 6. Markdown report.
    let mut report = format!("# Research: {topic}\n\n## Overview\n\n{overview}\n\n## Sources\n\n");
    for source in &sources {
        let n = source["n"].as_u64().unwrap_or_default();
        let title = source["title"].as_str().unwrap_or_default();
        let url = source["url"].as_str().unwrap_or_default();
        let summary = source["summary"].as_str().unwrap_or_default();
        report.push_str(&format!(
            "### [{n}] {title}\n\n- URL: {url}\n- Summary: {summary}\n\n"
        ));
    }
    if !failures.is_empty() {
        report.push_str("## Failed Sources\n\n");
        for failure in &failures {
            report.push_str(&format!(
                "- {} ({})\n",
                failure["url"].as_str().unwrap_or_default(),
                failure["error"].as_str().unwrap_or_default()
            ));
        }
        report.push('\n');
    }

    Ok(json!({
        "ok": true,
        "command": "Web_Research",
        "topic": topic,
        "report": report,
        "sources": sources,
        "failed": failures,
    }))
}

pub fn definition() -> ActResult<CommandDef> {
    CommandBuilder::new("Web_Research", "Deep research on a topic: multi-engine search, source ranking, parallel fetch, extractive per-source summaries and a cited Markdown report. Heuristic only (no LLM required).", Capability::Net, "wr")
        .param(Param::string("topic").alias("t").required())
        .param(Param::integer("max_sources").alias("s").alias("sources").min(1.0).max(20.0).default(json!(8)))
        .param(Param::integer("max_results").alias("m").alias("max").min(1.0).default(json!(15)))
        .param(Param::array_of_string("engines").alias("e").alias("engine"))
        .output_done(json!({
            "type": "object", "required": ["ok", "command", "topic", "report", "sources"],
            "properties": { "ok": { "type": "boolean" }, "command": { "const": "Web_Research" },
                "topic": { "type": "string" }, "report": { "type": "string", "description": "带编号引用的 Markdown 报告" },
                "sources": { "type": "array", "items": { "type": "object", "properties": {
                    "n": { "type": "integer" }, "title": { "type": "string" }, "url": { "type": "string" },
                    "final_url": { "type": "string" }, "bytes": { "type": "integer" },
                    "summary": { "type": "string", "description": "≤400 字符抽取式摘要" } } } },
                "failed": { "type": "array", "items": { "type": "object" }, "description": "抓取失败的源，不影响报告生成" } }
        }))
        .example("--topic \"mcp protocol adoption\" --sources 8")
        .bind(Arc::new(WebResearch))
}
