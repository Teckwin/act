//! Fs_CreateDir (batch create) / Fs_ListDir (single dir) / Fs_FileInfo (single path).

use std::sync::Arc;

use act_kernel::error::{ActError, ActResult};
use act_kernel::{
    builder::{CommandBuilder, Param, Verify},
    Capability, CommandDef, CommandHandler, SandboxContext,
};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use crate::util;

#[derive(Deserialize)]
struct CreateDirParams {
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
        let params: CreateDirParams = util::parse_params(params, "Fs_CreateDir")?;
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
    path: String,
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
        let cap = params
            .max_results
            .unwrap_or(ctx.config.limits.max_find_results);
        let depth = params
            .depth
            .unwrap_or(1)
            .min(ctx.config.limits.max_depth as u64);

        let (abs, _) = ctx.resolve_path(&params.path)?;
        if !abs.is_dir() {
            return Err(ActError::invalid_params(
                "Fs_ListDir",
                format!("'{}' is not a directory", params.path),
            ));
        }
        let mut entries = Vec::new();
        let mut truncated = false;
        for entry in walkdir::WalkDir::new(&abs)
            .max_depth(depth as usize)
            .sort_by_file_name()
            .into_iter()
            .filter_entry(|e| {
                if params.include_hidden {
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
                "path": util::join_user_path(&params.path, rel),
                "name": entry.file_name().to_string_lossy(),
                "is_dir": entry.path().is_dir(),
                "size": meta.as_ref().map(|m| m.len()).unwrap_or(0),
            }));
        }
        Ok(util::flat_ok(
            "Fs_ListDir",
            json!({
                "path": params.path,
                "entries": entries,
                "count": entries.len(),
                "truncated": truncated,
            }),
        ))
    }
}

#[derive(Deserialize)]
struct InfoParams {
    path: String,
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
        let (abs, _) = ctx.resolve_path(&params.path)?;
        let meta = std::fs::symlink_metadata(&abs).map_err(|e| {
            ActError::execution(
                "Fs_FileInfo",
                format!("cannot stat '{}': {}", params.path, e),
            )
        })?;
        let to_iso = |t: std::io::Result<std::time::SystemTime>| {
            t.ok()
                .map(|st| chrono::DateTime::<chrono::Utc>::from(st).to_rfc3339())
        };
        let abs_str = abs.to_string_lossy().replace('\\', "/");
        Ok(util::flat_ok(
            "Fs_FileInfo",
            json!({
                "path": params.path,
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
}

pub fn create_dir_definition() -> ActResult<CommandDef> {
    CommandBuilder::new("Fs_CreateDir", "Batch-create directories including parents (mkdir -p).", Capability::Write, "cd")
        .param(Param::array_of_string("paths").alias("ps").required().verify(Verify::PathLike).desc("批量新建（场景支撑）"))
        .output_done(json!({
            "type": "object", "required": ["ok", "command", "results", "summary"],
            "properties": { "ok": { "type": "boolean" }, "command": { "const": "Fs_CreateDir" },
                "results": { "type": "array", "items": { "type": "object", "properties": {
                    "path": { "type": "string" }, "ok": { "type": "boolean" }, "created": { "type": "boolean" } } } },
                "summary": { "type": "object" } }
        }))
        .example("--paths assets/img,assets/fonts,docs/zh")
        .bind(Arc::new(CreateDir))
}

pub fn list_dir_definition() -> ActResult<CommandDef> {
    CommandBuilder::new("Fs_ListDir", "List ONE directory with depth control; hidden entries skipped unless include_hidden.", Capability::Read, "ls")
        .param(Param::string("path").alias("p").required().verify(Verify::PathLike).desc("目录（单个）"))
        .param(Param::integer("depth").alias("d").min(1.0).max(64.0).default(json!(1)))
        .param(Param::boolean("include_hidden").alias("h").alias("hidden").default(json!(false)))
        .param(Param::integer("max_results").alias("m").alias("max").min(1.0).max(500.0))
        .output_done(json!({
            "type": "object", "required": ["ok", "command", "path", "entries", "count"],
            "properties": { "ok": { "type": "boolean" }, "command": { "const": "Fs_ListDir" },
                "path": { "type": "string" },
                "entries": { "type": "array", "items": { "type": "object", "properties": {
                    "path": { "type": "string", "description": "输入路径+相对子路径，可直接回传其他命令" },
                    "name": { "type": "string" }, "is_dir": { "type": "boolean" }, "size": { "type": "integer" } } } },
                "count": { "type": "integer" }, "truncated": { "type": "boolean" } }
        }))
        .example("--path src --depth 2")
        .bind(Arc::new(ListDir))
}

pub fn file_info_definition() -> ActResult<CommandDef> {
    CommandBuilder::new("Fs_FileInfo", "Stat ONE file/directory: type, size, timestamps, readonly flag.", Capability::Read, "fi")
        .param(Param::string("path").alias("p").required().verify(Verify::PathLike).desc("目标路径（单个）"))
        .output_done(json!({
            "type": "object", "required": ["ok", "command", "path", "size", "is_file"],
            "properties": { "ok": { "type": "boolean" }, "command": { "const": "Fs_FileInfo" },
                "path": { "type": "string" }, "abs_path": { "type": "string" },
                "exists": { "type": "boolean" }, "is_file": { "type": "boolean" }, "is_dir": { "type": "boolean" },
                "is_symlink": { "type": "boolean" }, "size": { "type": "integer" },
                "modified": { "type": "string", "description": "RFC3339 或 null" },
                "created": { "type": "string", "description": "RFC3339 或 null" },
                "readonly": { "type": "boolean" } }
        }))
        .example("--path Cargo.toml")
        .bind(Arc::new(FileInfo))
}
