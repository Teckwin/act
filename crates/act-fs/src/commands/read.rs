//! Fs_ReadFile: batch read with encoding detection and line slicing.

use std::sync::Arc;

use act_kernel::error::{ActError, ActResult};
use act_kernel::{Capability, CommandDef, CommandHandler, SandboxContext};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use crate::{encoding, util};

#[derive(Deserialize)]
struct Params {
    paths: Vec<String>,
    offset: Option<u64>,
    limit: Option<u64>,
}

pub struct ReadFile;

#[async_trait]
impl CommandHandler for ReadFile {
    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: &SandboxContext,
    ) -> ActResult<serde_json::Value> {
        let params: Params = util::parse_params(params, "Fs_ReadFile")?;
        if params.paths.is_empty() {
            return Err(ActError::invalid_params(
                "Fs_ReadFile",
                "'paths' must not be empty",
            ));
        }
        let max_read = ctx.config.limits.max_read_bytes;
        let mut results = Vec::with_capacity(params.paths.len());

        for input in &params.paths {
            let item = match read_one(ctx, input, params.offset, params.limit, max_read) {
                Ok(value) => value,
                Err(err) => util::err_item(input, &err),
            };
            results.push(item);
        }
        Ok(util::envelope("Fs_ReadFile", results))
    }
}

fn read_one(
    ctx: &SandboxContext,
    input: &str,
    offset: Option<u64>,
    limit: Option<u64>,
    max_read: u64,
) -> ActResult<serde_json::Value> {
    let (abs, _) = ctx.resolve_path(input)?;
    let meta = std::fs::metadata(&abs).map_err(|e| {
        ActError::execution("Fs_ReadFile", format!("cannot stat '{}': {}", input, e))
    })?;
    if meta.is_dir() {
        return Err(ActError::invalid_params(
            "Fs_ReadFile",
            format!("'{}' is a directory (use Fs_ListDir)", input),
        ));
    }
    if meta.len() > max_read && offset.is_none() && limit.is_none() {
        return Err(ActError::LimitExceeded {
            reason: format!(
                "file '{}' is {} bytes (cap {}); request offset/limit to slice lines",
                input,
                meta.len(),
                max_read
            ),
        });
    }
    let bytes = std::fs::read(&abs).map_err(|e| {
        ActError::execution("Fs_ReadFile", format!("cannot read '{}': {}", input, e))
    })?;
    let decoded = encoding::decode(&bytes, &ctx.config.fs.fallback_encoding);

    let total_lines = decoded.text.lines().count() as u64;
    let (content, line_start, line_end) = match (offset, limit) {
        (None, None) => (decoded.text.clone(), 0u64, total_lines.saturating_sub(1)),
        _ => {
            let start = offset.unwrap_or(0);
            let lines: Vec<&str> = decoded.text.lines().collect();
            let begin = (start as usize).min(lines.len());
            let take = limit.map(|l| l as usize).unwrap_or(lines.len() - begin);
            let end = (begin + take).min(lines.len());
            (
                lines[begin..end].join("\n"),
                start,
                (end as u64).saturating_sub(1),
            )
        }
    };

    Ok(util::ok_item(
        input,
        json!({
            "encoding": decoded.encoding,
            "lossy": decoded.lossy,
            "size": meta.len(),
            "total_lines": total_lines,
            "line_start": line_start,
            "line_end": line_end,
            "content": content,
        }),
    ))
}

pub fn definition() -> ActResult<CommandDef> {
    CommandDef::new(
        "Fs_ReadFile",
        "Read one or more text files with automatic encoding detection (UTF-8/BOM/UTF-16/fallback). Supports line offset/limit slicing.",
        Capability::Read,
        json!({
            "type": "object",
            "properties": {
                "paths": { "type": "array", "items": { "type": "string" }, "description": "File paths (batch)" },
                "offset": { "type": "integer", "description": "0-based start line" },
                "limit": { "type": "integer", "description": "Max lines to return" }
            },
            "required": ["paths"]
        }),
        vec!["/paths/*".into()],
        vec![],
        Arc::new(ReadFile),
    )
}
