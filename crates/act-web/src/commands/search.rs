//! Web_Search: multi-engine parallel search with RRF merge and dedupe.

use std::sync::Arc;

use act_kernel::error::{ActError, ActResult};
use act_kernel::{
    builder::{CommandBuilder, Param},
    Capability, CommandDef, CommandHandler, SandboxContext,
};
use async_trait::async_trait;
use futures::StreamExt;
use serde::Deserialize;
use serde_json::json;

use crate::engines::{self, SearchEngine};

#[derive(Deserialize)]
struct Params {
    queries: Vec<String>,
    #[serde(default)]
    engines: Option<Vec<String>>,
    #[serde(default)]
    max_results: Option<usize>,
}

pub struct WebSearch;

#[async_trait]
impl CommandHandler for WebSearch {
    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: &SandboxContext,
    ) -> ActResult<serde_json::Value> {
        let params: Params = serde_json::from_value(params)
            .map_err(|e| ActError::invalid_params("Web_Search", e.to_string()))?;
        if params.queries.is_empty() {
            return Err(ActError::invalid_params(
                "Web_Search",
                "'queries' must not be empty",
            ));
        }
        let count = params.max_results.unwrap_or(10);
        let engine_list = engines::available(ctx);
        let selected: Vec<Arc<dyn SearchEngine>> = match &params.engines {
            Some(requested) => {
                let picked: Vec<Arc<dyn SearchEngine>> = engine_list
                    .into_iter()
                    .filter(|e| requested.iter().any(|r| r == e.name()))
                    .collect();
                if picked.is_empty() {
                    return Err(ActError::invalid_params(
                        "Web_Search",
                        format!(
                            "no requested engines available; available: [{}]",
                            engines::available(ctx)
                                .iter()
                                .map(|e| e.name().to_string())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ),
                    ));
                }
                picked
            }
            None => engine_list,
        };
        Ok(run_search(ctx, &params.queries, &selected, count).await)
    }
}

/// Core search logic (also used by Web_Research and tests).
pub async fn run_search(
    ctx: &SandboxContext,
    queries: &[String],
    engine_list: &[Arc<dyn SearchEngine>],
    count: usize,
) -> serde_json::Value {
    let concurrency = ctx.config.limits.web_concurrency;
    let jobs: Vec<(String, String)> = queries
        .iter()
        .flat_map(|q| {
            engine_list
                .iter()
                .map(move |e| (q.clone(), e.name().to_string()))
        })
        .collect();

    let outcomes: Vec<((String, String), Result<Vec<engines::SearchHit>, ActError>)> =
        futures::stream::iter(jobs)
            .map(|(query, engine_name)| {
                let engine = engine_list
                    .iter()
                    .find(|e| e.name() == engine_name)
                    .expect("engine");
                async move {
                    let result = engine.search(ctx, &query, count).await;
                    ((query, engine_name), result)
                }
            })
            .buffer_unordered(concurrency)
            .collect()
            .await;

    // Group per query.
    let mut per_query: Vec<serde_json::Value> = Vec::new();
    for query in queries {
        let mut engine_results: Vec<(String, Vec<engines::SearchHit>)> = Vec::new();
        let mut warnings: Vec<serde_json::Value> = Vec::new();
        for ((q, engine_name), result) in &outcomes {
            if q != query {
                continue;
            }
            match result {
                Ok(hits) => engine_results.push((engine_name.clone(), hits.clone())),
                Err(err) => warnings.push(json!({
                    "engine": engine_name,
                    "error": err.to_string(),
                })),
            }
        }
        let merged = engines::merge(&engine_results, count);
        per_query.push(json!({
            "query": query,
            "results": merged,
            "count": merged.len(),
            "engines_used": engine_results.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>(),
            "warnings": warnings,
        }));
    }

    json!({
        "ok": true,
        "command": "Web_Search",
        "queries": per_query,
    })
}

pub fn definition() -> ActResult<CommandDef> {
    CommandBuilder::new("Web_Search", "Search the web across multiple engines in parallel (DuckDuckGo/Bing without keys; Google/Brave/SearXNG optional) and merge results with Reciprocal Rank Fusion dedupe.", Capability::Net, "ws")
        .param(Param::array_of_string("queries").alias("q").required().desc("多查询并行（场景支撑）"))
        .param(Param::array_of_string("engines").alias("e").alias("engine").desc("可用引擎子集；不可用引擎报 invalid_params"))
        .param(Param::integer("max_results").alias("m").alias("max").min(1.0).max(500.0).default(json!(10)))
        .output_done(json!({
            "type": "object", "required": ["ok", "command", "queries"],
            "properties": { "ok": { "type": "boolean" }, "command": { "const": "Web_Search" },
                "queries": { "type": "array", "items": { "type": "object", "properties": {
                    "query": { "type": "string" },
                    "results": { "type": "array", "items": { "type": "object", "properties": {
                        "rank": { "type": "integer" }, "title": { "type": "string" }, "url": { "type": "string" },
                        "snippet": { "type": "string" }, "engines": { "type": "array", "items": { "type": "string" } }, "score": { "type": "number" } } } },
                    "count": { "type": "integer" }, "engines_used": { "type": "array", "items": { "type": "string" } },
                    "warnings": { "type": "array", "items": { "type": "object" }, "description": "单引擎失败降级，不影响整体" } } } } }
        }))
        .example("--queries \"rust mcp server\" --engines duckduckgo,bing --max 10")
        .bind(Arc::new(WebSearch))
}
