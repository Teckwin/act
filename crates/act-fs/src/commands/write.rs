//! Fs_WriteFile / Fs_AppendFile / Fs_CreateFile.

use std::sync::Arc;

use act_kernel::error::{ActError, ActResult};
use act_kernel::{Capability, CommandDef, CommandHandler, SandboxContext};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use crate::{encoding, util};

#[derive(Deserialize)]
struct WriteParams {
    files: Vec<FileSpec>,
}

#[derive(Deserialize)]
struct FileSpec {
    path: String,
    #[serde(default)]
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
        if params.files.is_empty() {
            return Err(ActError::invalid_params(
                "Fs_WriteFile",
                "'files' must not be empty",
            ));
        }
        let max_write = ctx.config.limits.max_write_bytes;
        let mut results = Vec::with_capacity(params.files.len());
        for spec in &params.files {
            let item = match write_one(ctx, spec, max_write, false) {
                Ok(value) => value,
                Err(err) => util::err_item(&spec.path, &err),
            };
            results.push(item);
        }
        Ok(util::envelope("Fs_WriteFile", results))
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
        if params.files.is_empty() {
            return Err(ActError::invalid_params(
                "Fs_AppendFile",
                "'files' must not be empty",
            ));
        }
        let max_write = ctx.config.limits.max_write_bytes;
        let mut results = Vec::with_capacity(params.files.len());
        for spec in &params.files {
            let item = match write_one(ctx, spec, max_write, true) {
                Ok(value) => value,
                Err(err) => util::err_item(&spec.path, &err),
            };
            results.push(item);
        }
        Ok(util::envelope("Fs_AppendFile", results))
    }
}

fn write_one(
    ctx: &SandboxContext,
    spec: &FileSpec,
    max_write: u64,
    append: bool,
) -> ActResult<serde_json::Value> {
    let (abs, _) = ctx.resolve_path(&spec.path)?;
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
            .map_err(|e| {
                ActError::execution("Fs_AppendFile", format!("open '{}': {}", spec.path, e))
            })?;
        file.write_all(&bytes).map_err(ActError::Io)?;
        return Ok(util::ok_item(
            &spec.path,
            json!({ "appended_bytes": bytes.len(), "encoding": enc_name }),
        ));
    }
    util::atomic_write(&abs, &bytes).map_err(|e| {
        ActError::execution("Fs_WriteFile", format!("write '{}': {}", spec.path, e))
    })?;
    Ok(util::ok_item(
        &spec.path,
        json!({ "bytes": bytes.len(), "encoding": enc_name }),
    ))
}

#[derive(Deserialize)]
struct CreateParams {
    paths: Vec<String>,
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
        let mut results = Vec::with_capacity(params.paths.len());
        for input in &params.paths {
            let item = match create_one(ctx, input) {
                Ok(value) => value,
                Err(err) => util::err_item(input, &err),
            };
            results.push(item);
        }
        Ok(util::envelope("Fs_CreateFile", results))
    }
}

fn create_one(ctx: &SandboxContext, input: &str) -> ActResult<serde_json::Value> {
    let (abs, _) = ctx.resolve_path(input)?;
    if abs.exists() {
        return Ok(util::ok_item(input, json!({ "created": false })));
    }
    util::atomic_write(&abs, b"")
        .map_err(|e| ActError::execution("Fs_CreateFile", format!("create '{}': {}", input, e)))?;
    Ok(util::ok_item(input, json!({ "created": true })))
}

pub fn write_definition() -> ActResult<CommandDef> {
    CommandDef::new(
        "Fs_WriteFile",
        "Create or overwrite files (batch). Atomic write; encoding and EOL configurable; defaults to UTF-8.",
        Capability::Write,
        json!({
            "type": "object",
            "properties": {
                "files": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string" },
                            "content": { "type": "string" },
                            "encoding": { "type": "string", "description": "utf-8 (default) | utf-8-bom | utf-16le | utf-16be | gbk ..." },
                            "eol": { "type": "string", "description": "auto (default) | lf | crlf | none" }
                        },
                        "required": ["path", "content"]
                    }
                }
            },
            "required": ["files"]
        }),
        vec!["/files/*/path".into()],
        vec![],
        Arc::new(WriteFile),
    )
}

pub fn append_definition() -> ActResult<CommandDef> {
    CommandDef::new(
        "Fs_AppendFile",
        "Append content to files (batch); creates missing files.",
        Capability::Write,
        json!({
            "type": "object",
            "properties": {
                "files": {
                    "type": "array",
                    "items": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string" },
                            "content": { "type": "string" },
                            "encoding": { "type": "string" }
                        },
                        "required": ["path", "content"]
                    }
                }
            },
            "required": ["files"]
        }),
        vec!["/files/*/path".into()],
        vec![],
        Arc::new(AppendFile),
    )
}

pub fn create_definition() -> ActResult<CommandDef> {
    CommandDef::new(
        "Fs_CreateFile",
        "Create empty files (touch, batch); parents are created; existing files are left untouched.",
        Capability::Write,
        json!({
            "type": "object",
            "properties": { "paths": { "type": "array", "items": { "type": "string" } } },
            "required": ["paths"]
        }),
        vec!["/paths/*".into()],
        vec![],
        Arc::new(CreateFile),
    )
}
