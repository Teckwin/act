//! Fs_WriteFile / Fs_AppendFile (single file) + Fs_CreateFile (batch create).

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
struct WriteParams {
    path: String,
    content: String,
    #[serde(default)]
    encoding: Option<String>,
    #[serde(default)]
    eol: Option<String>,
}

pub struct WriteFile;

#[async_trait]
impl CommandHandler for WriteFile {
    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: &SandboxContext,
    ) -> ActResult<serde_json::Value> {
        let params: WriteParams = util::parse_params(params, "Fs_WriteFile")?;
        write_one(ctx, &params, false)
    }
}

pub struct AppendFile;

#[async_trait]
impl CommandHandler for AppendFile {
    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: &SandboxContext,
    ) -> ActResult<serde_json::Value> {
        let params: WriteParams = util::parse_params(params, "Fs_AppendFile")?;
        write_one(ctx, &params, true)
    }
}

fn write_one(
    ctx: &SandboxContext,
    spec: &WriteParams,
    append: bool,
) -> ActResult<serde_json::Value> {
    let command = if append {
        "Fs_AppendFile"
    } else {
        "Fs_WriteFile"
    };
    let (abs, _) = ctx.resolve_path(&spec.path)?;
    let max_write = ctx.config.limits.max_write_bytes;
    let eol_mode = spec.eol.as_deref().unwrap_or(&ctx.config.fs.eol);
    let text = encoding::normalize_eol(&spec.content, eol_mode);
    let enc_name = spec.encoding.as_deref().unwrap_or("utf-8");
    let bytes = encoding::encode(&text, enc_name)?;
    if bytes.len() as u64 > max_write {
        return Err(ActError::LimitExceeded {
            reason: format!("content is {} bytes (cap {})", bytes.len(), max_write),
        });
    }
    if append {
        use std::io::Write;
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent).map_err(ActError::Io)?;
        }
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&abs)
            .map_err(|e| ActError::execution(command, format!("open '{}': {}", spec.path, e)))?;
        file.write_all(&bytes).map_err(ActError::Io)?;
        return Ok(util::flat_ok(
            command,
            json!({ "path": spec.path, "appended_bytes": bytes.len(), "encoding": enc_name }),
        ));
    }
    util::atomic_write(&abs, &bytes)
        .map_err(|e| ActError::execution(command, format!("write '{}': {}", spec.path, e)))?;
    Ok(util::flat_ok(
        command,
        json!({ "path": spec.path, "bytes": bytes.len(), "encoding": enc_name }),
    ))
}

#[derive(Deserialize)]
struct CreateParams {
    paths: Vec<String>,
    #[serde(default)]
    content: Option<String>,
}

pub struct CreateFile;

#[async_trait]
impl CommandHandler for CreateFile {
    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: &SandboxContext,
    ) -> ActResult<serde_json::Value> {
        let params: CreateParams = util::parse_params(params, "Fs_CreateFile")?;
        if params.paths.is_empty() {
            return Err(ActError::invalid_params(
                "Fs_CreateFile",
                "'paths' must not be empty",
            ));
        }
        let max_write = ctx.config.limits.max_write_bytes;
        let mut results = Vec::with_capacity(params.paths.len());
        for input in &params.paths {
            let item = match create_one(ctx, input, params.content.as_deref(), max_write) {
                Ok(value) => value,
                Err(err) => util::err_item(input, &err),
            };
            results.push(item);
        }
        Ok(util::envelope("Fs_CreateFile", results))
    }
}

fn create_one(
    ctx: &SandboxContext,
    input: &str,
    content: Option<&str>,
    max_write: u64,
) -> ActResult<serde_json::Value> {
    let (abs, _) = ctx.resolve_path(input)?;
    if abs.exists() {
        return Ok(util::ok_item(input, json!({ "created": false })));
    }
    let bytes = match content {
        Some(text) => {
            let text = encoding::normalize_eol(text, &ctx.config.fs.eol);
            let bytes = encoding::encode(&text, "utf-8")?;
            if bytes.len() as u64 > max_write {
                return Err(ActError::LimitExceeded {
                    reason: format!("content is {} bytes (cap {})", bytes.len(), max_write),
                });
            }
            bytes
        }
        None => Vec::new(),
    };
    util::atomic_write(&abs, &bytes)
        .map_err(|e| ActError::execution("Fs_CreateFile", format!("create '{}': {}", input, e)))?;
    Ok(util::ok_item(input, json!({ "created": true })))
}

pub fn write_definition() -> ActResult<CommandDef> {
    CommandBuilder::new("Fs_WriteFile", "Create or overwrite ONE file (atomic write; encoding/EOL configurable; default UTF-8).", Capability::Write, "fw")
        .param(Param::string("path").alias("p").required().verify(Verify::PathLike).desc("目标文件（单文件）"))
        .param(Param::string("content").alias("c").required().desc("全文内容（整体替换）"))
        .param(Param::string("encoding").alias("en").default(json!("utf-8")).desc("utf-8 | utf-8-bom | utf-16le | utf-16be | gbk …"))
        .param(Param::string("eol").alias("eo").enum_values(&["auto", "lf", "crlf", "none"]).default(json!("auto")))
        .output_done(json!({
            "type": "object", "required": ["ok", "command", "path", "bytes"],
            "properties": { "ok": { "type": "boolean" }, "command": { "const": "Fs_WriteFile" },
                "path": { "type": "string" }, "bytes": { "type": "integer" }, "encoding": { "type": "string" } }
        }))
        .example("--path src/new.rs --content \"fn main() {}\"")
        .bind(Arc::new(WriteFile))
}

pub fn append_definition() -> ActResult<CommandDef> {
    CommandBuilder::new("Fs_AppendFile", "Append content to ONE file; creates it when missing.", Capability::Write, "fa")
        .param(Param::string("path").alias("p").required().verify(Verify::PathLike))
        .param(Param::string("content").alias("c").required())
        .param(Param::string("encoding").alias("en").default(json!("utf-8")))
        .output_done(json!({
            "type": "object", "required": ["ok", "command", "path", "appended_bytes"],
            "properties": { "ok": { "type": "boolean" }, "command": { "const": "Fs_AppendFile" },
                "path": { "type": "string" }, "appended_bytes": { "type": "integer" }, "encoding": { "type": "string" } }
        }))
        .example("--path logs/run.log --content \"line\n\"")
        .bind(Arc::new(AppendFile))
}

pub fn create_definition() -> ActResult<CommandDef> {
    CommandBuilder::new("Fs_CreateFile", "Batch-create empty files (touch); parents auto-created; existing files untouched. Optional identical content for all.", Capability::Write, "fc")
        .param(Param::array_of_string("paths").alias("ps").required().verify(Verify::PathLike).desc("批量新建（唯一批量场景之一）"))
        .param(Param::string("content").alias("c").desc("可选：为所有新文件写入相同初始内容"))
        .output_done(json!({
            "type": "object", "required": ["ok", "command", "results", "summary"],
            "properties": { "ok": { "type": "boolean" }, "command": { "const": "Fs_CreateFile" },
                "results": { "type": "array", "items": { "type": "object", "properties": {
                    "path": { "type": "string" }, "ok": { "type": "boolean" }, "created": { "type": "boolean" } } } },
                "summary": { "type": "object", "properties": { "succeeded": { "type": "integer" }, "failed": { "type": "integer" } } } }
        }))
        .example("--paths src/a.rs,src/b.rs,docs/c.md")
        .bind(Arc::new(CreateFile))
}
