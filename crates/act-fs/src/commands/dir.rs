//! Fs_CreateDir / Fs_ListDir / Fs_FileInfo.

use std::sync::Arc;

use act_kernel::error::{ActError, ActResult};
use act_kernel::{Capability, CommandDef, CommandHandler, SandboxContext};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use crate::util;

#[derive(Deserialize)]
struct PathsParams {
    paths: Vec<String>,
}

pub struct CreateDir;

#[async_trait]
impl CommandHandler for CreateDir {
    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: &SandboxContext,
    ) -> ActResult<serde_json::Value> {
        let params: PathsParams = util::parse_params(params, "Fs_CreateDir")?;
        if params.paths.is_empty() {
            return Err(ActError::invalid_params(
                "Fs_CreateDir",
                "'paths' must not be empty",
            ));
        }
        let mut results = Vec::with_capacity(params.paths.len());
        for input in &params.paths {
            let item = match ctx.resolve_path(input) {
                Ok((abs, _)) => {
                    if abs.is_dir() {
                        util::ok_item(input, json!({ "created": false }))
                    } else {
                        std::fs::create_dir_all(&abs)
                            .map_err(|e| {
                                ActError::execution(
                                    "Fs_CreateDir",
                                    format!("mkdir '{}': {}", input, e),
                                )
                            })
                            .map(|_| util::ok_item(input, json!({ "created": true })))?
                    }
                }
                Err(err) => util::err_item(input, &err),
            };
            results.push(item);
        }
        Ok(util::envelope("Fs_CreateDir", results))
    }
}

#[derive(Deserialize)]
struct ListParams {
    paths: Vec<String>,
    #[serde(default)]
    depth: Option<u64>,
    #[serde(default)]
    include_hidden: bool,
    #[serde(default)]
    max_results: Option<usize>,
}

pub struct ListDir;

#[async_trait]
impl CommandHandler for ListDir {
    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: &SandboxContext,
    ) -> ActResult<serde_json::Value> {
        let params: ListParams = util::parse_params(params, "Fs_ListDir")?;
        if params.paths.is_empty() {
            return Err(ActError::invalid_params(
                "Fs_ListDir",
                "'paths' must not be empty",
            ));
        }
        let cap = params
            .max_results
            .unwrap_or(ctx.config.limits.max_find_results);
        let depth = params
            .depth
            .unwrap_or(1)
            .min(ctx.config.limits.max_depth as u64);
        let mut results = Vec::with_capacity(params.paths.len());

        for input in &params.paths {
            let item = match list_one(ctx, input, depth as usize, params.include_hidden, cap) {
                Ok(value) => value,
                Err(err) => util::err_item(input, &err),
            };
            results.push(item);
        }
        Ok(util::envelope("Fs_ListDir", results))
    }
}

fn list_one(
    ctx: &SandboxContext,
    input: &str,
    depth: usize,
    include_hidden: bool,
    cap: usize,
) -> ActResult<serde_json::Value> {
    let (abs, _) = ctx.resolve_path(input)?;
    if !abs.is_dir() {
        return Err(ActError::invalid_params(
            "Fs_ListDir",
            format!("'{}' is not a directory", input),
        ));
    }
    let mut entries = Vec::new();
    let mut truncated = false;
    for entry in walkdir::WalkDir::new(&abs)
        .max_depth(depth)
        .sort_by_file_name()
        .into_iter()
        .filter_entry(|e| {
            if include_hidden {
                true
            } else {
                e.depth() == 0 || !e.file_name().to_string_lossy().starts_with('.')
            }
        })
        .filter_map(|e| e.ok())
    {
        if entry.depth() == 0 {
            continue;
        }
        if entries.len() >= cap {
            truncated = true;
            break;
        }
        let rel = entry.path().strip_prefix(&abs).unwrap_or(entry.path());
        let meta = entry.metadata().ok();
        entries.push(json!({
            "path": util::join_user_path(input, rel),
            "name": entry.file_name().to_string_lossy(),
            "is_dir": entry.path().is_dir(),
            "size": meta.as_ref().map(|m| m.len()).unwrap_or(0),
        }));
    }
    let _ = ctx;
    Ok(util::ok_item(
        input,
        json!({ "entries": entries, "count": entries.len(), "truncated": truncated }),
    ))
}

#[derive(Deserialize)]
struct InfoParams {
    paths: Vec<String>,
}

pub struct FileInfo;

#[async_trait]
impl CommandHandler for FileInfo {
    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: &SandboxContext,
    ) -> ActResult<serde_json::Value> {
        let params: InfoParams = util::parse_params(params, "Fs_FileInfo")?;
        if params.paths.is_empty() {
            return Err(ActError::invalid_params(
                "Fs_FileInfo",
                "'paths' must not be empty",
            ));
        }
        let mut results = Vec::with_capacity(params.paths.len());
        for input in &params.paths {
            let item = match info_one(ctx, input) {
                Ok(value) => value,
                Err(err) => util::err_item(input, &err),
            };
            results.push(item);
        }
        Ok(util::envelope("Fs_FileInfo", results))
    }
}

fn info_one(ctx: &SandboxContext, input: &str) -> ActResult<serde_json::Value> {
    let (abs, _) = ctx.resolve_path(input)?;
    let meta = std::fs::symlink_metadata(&abs).map_err(|e| {
        ActError::execution("Fs_FileInfo", format!("cannot stat '{}': {}", input, e))
    })?;
    let to_iso = |t: std::io::Result<std::time::SystemTime>| {
        t.ok()
            .map(|st| chrono::DateTime::<chrono::Utc>::from(st).to_rfc3339())
    };
    let abs_str = abs.to_string_lossy().replace('\\', "/");
    Ok(util::ok_item(
        input,
        json!({
            "abs_path": abs_str,
            "exists": true,
            "is_file": meta.is_file(),
            "is_dir": meta.is_dir(),
            "is_symlink": meta.file_type().is_symlink(),
            "size": meta.len(),
            "modified": to_iso(meta.modified()),
            "created": to_iso(meta.created()),
            "readonly": meta.permissions().readonly(),
        }),
    ))
}

pub fn create_dir_definition() -> ActResult<CommandDef> {
    CommandDef::new(
        "Fs_CreateDir",
        "Create directories including parents (mkdir -p, batch).",
        Capability::Write,
        json!({
            "type": "object",
            "properties": { "paths": { "type": "array", "items": { "type": "string" } } },
            "required": ["paths"]
        }),
        vec!["/paths/*".into()],
        vec![],
        Arc::new(CreateDir),
    )
}

pub fn list_dir_definition() -> ActResult<CommandDef> {
    CommandDef::new(
        "Fs_ListDir",
        "List directory contents (batch) with depth control; hidden entries are skipped unless include_hidden=true.",
        Capability::Read,
        json!({
            "type": "object",
            "properties": {
                "paths": { "type": "array", "items": { "type": "string" } },
                "depth": { "type": "integer", "default": 1 },
                "include_hidden": { "type": "boolean", "default": false },
                "max_results": { "type": "integer" }
            },
            "required": ["paths"]
        }),
        vec!["/paths/*".into()],
        vec![],
        Arc::new(ListDir),
    )
}

pub fn file_info_definition() -> ActResult<CommandDef> {
    CommandDef::new(
        "Fs_FileInfo",
        "Stat files/directories (batch): type, size, timestamps, readonly flag.",
        Capability::Read,
        json!({
            "type": "object",
            "properties": { "paths": { "type": "array", "items": { "type": "string" } } },
            "required": ["paths"]
        }),
        vec!["/paths/*".into()],
        vec![],
        Arc::new(FileInfo),
    )
}
