//! Fs_ReadFile: single-file read with encoding detection and line slicing.

use std::sync::Arc;

use act_kernel::error::{ActError, ActResult};
use act_kernel::{
    builder::{CommandBuilder, Param, Verify},
    Capability, CommandDef, CommandHandler, SandboxContext,
};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use crate::{encoding, util};

#[derive(Deserialize)]
struct Params {
    path: String,
    #[serde(default)]
    offset: Option<u64>,
    #[serde(default)]
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
        read_one(
            ctx,
            &params.path,
            params.offset,
            params.limit,
            ctx.config.limits.max_read_bytes,
        )
    }
}

pub fn read_one(
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

    Ok(util::flat_ok(
        "Fs_ReadFile",
        json!({
            "path": input,
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
    CommandBuilder::new(
        "Fs_ReadFile",
        "Read one text file with automatic encoding detection (UTF-8/BOM/UTF-16/fallback); optional line slicing.",
        Capability::Read,
        "fr",
    )
    .param(Param::string("path").alias("p").required().verify(Verify::PathLike).desc("File path (single file)"))
    .param(Param::integer("offset").alias("o").min(0.0).desc("0-based start line"))
    .param(Param::integer("limit").alias("l").min(1.0).desc("Max lines to return"))
    .output_done(json!({
        "type": "object",
        "required": ["ok", "command", "path", "content"],
        "properties": {
            "ok": { "type": "boolean" }, "command": { "const": "Fs_ReadFile" },
            "path": { "type": "string" },
            "encoding": { "type": "string", "description": "detected: utf-8 | utf-8-bom | utf-16le | utf-16be | gbk …" },
            "lossy": { "type": "boolean", "description": "true = 不可逆替换过" },
            "size": { "type": "integer" }, "total_lines": { "type": "integer" },
            "line_start": { "type": "integer" }, "line_end": { "type": "integer" },
            "content": { "type": "string" }
        }
    }))
    .output_fail("permission_denied", json!({
        "type": "object",
        "description": "CLI: error[permission_denied] exit=2; MCP: isError=true {code,message}",
        "properties": { "code": { "const": "permission_denied" }, "message": { "type": "string" } }
    }))
    .example("--path src/main.rs --offset 0 --limit 200")
    .bind(Arc::new(ReadFile))
}
