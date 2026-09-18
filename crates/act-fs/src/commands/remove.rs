//! Fs_RemoveFile / Fs_RemoveDir (single target, trash-first).

use std::sync::Arc;

use act_kernel::error::{ActError, ActResult};
use act_kernel::{
    builder::{CommandBuilder, Param, Verify},
    Capability, CommandDef, CommandHandler, SandboxContext,
};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use crate::{trash::Trash, util};

#[derive(Deserialize)]
struct RemoveParams {
    path: String,
    #[serde(default)]
    recursive: bool,
}

pub struct RemoveFile;

#[async_trait]
impl CommandHandler for RemoveFile {
    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: &SandboxContext,
    ) -> ActResult<serde_json::Value> {
        let params: RemoveParams = util::parse_params(params, "Fs_RemoveFile")?;
        remove_one(ctx, &params.path, false, false)
    }
}

pub struct RemoveDir;

#[async_trait]
impl CommandHandler for RemoveDir {
    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: &SandboxContext,
    ) -> ActResult<serde_json::Value> {
        let params: RemoveParams = util::parse_params(params, "Fs_RemoveDir")?;
        remove_one(ctx, &params.path, true, params.recursive)
    }
}

fn remove_one(
    ctx: &SandboxContext,
    input: &str,
    dir: bool,
    recursive: bool,
) -> ActResult<serde_json::Value> {
    let command = if dir { "Fs_RemoveDir" } else { "Fs_RemoveFile" };
    let (abs, root_idx) = ctx.resolve_path(input)?;
    util::ensure_not_root(ctx, &abs)?;

    let meta = std::fs::symlink_metadata(&abs)
        .map_err(|e| ActError::execution(command, format!("cannot stat '{}': {}", input, e)))?;
    if dir {
        if !meta.is_dir() {
            return Err(ActError::invalid_params(
                "Fs_RemoveDir",
                format!("'{}' is not a directory", input),
            ));
        }
        let non_empty = std::fs::read_dir(&abs)
            .map(|mut it| it.next().is_some())
            .unwrap_or(false);
        if non_empty && !recursive {
            return Err(ActError::invalid_params(
                "Fs_RemoveDir",
                format!("'{}' is not empty; set recursive=true", input),
            ));
        }
    } else if meta.is_dir() && !meta.file_type().is_symlink() {
        return Err(ActError::invalid_params(
            "Fs_RemoveFile",
            format!("'{}' is a directory (use Fs_RemoveDir)", input),
        ));
    }

    if Trash::enabled(ctx) {
        let trashed = Trash::stash(ctx, &abs, root_idx)?;
        let rel = ctx
            .rel_to_root(&trashed)
            .map(|p| p.to_string_lossy().replace('\\', "/"));
        return Ok(util::flat_ok(
            command,
            json!({ "path": input, "deleted": true, "mode": "trash", "trash_path": rel }),
        ));
    }
    if dir {
        std::fs::remove_dir_all(&abs)
            .map_err(|e| ActError::execution(command, format!("remove '{}': {}", input, e)))?;
    } else {
        std::fs::remove_file(&abs)
            .map_err(|e| ActError::execution(command, format!("remove '{}': {}", input, e)))?;
    }
    Ok(util::flat_ok(
        command,
        json!({ "path": input, "deleted": true, "mode": "permanent" }),
    ))
}

pub fn remove_file_definition() -> ActResult<CommandDef> {
    CommandBuilder::new("Fs_RemoveFile", "Delete ONE file. Default mode moves it to .act/trash/ (recoverable); fs.delete_mode=permanent for hard delete.", Capability::Write, "rf")
        .param(Param::string("path").alias("p").required().verify(Verify::PathLike).desc("目标文件（单个）"))
        .output_done(json!({
            "type": "object", "required": ["ok", "command", "path", "deleted", "mode"],
            "properties": { "ok": { "type": "boolean" }, "command": { "const": "Fs_RemoveFile" },
                "path": { "type": "string" }, "deleted": { "type": "boolean" },
                "mode": { "enum": ["trash", "permanent"] },
                "trash_path": { "type": "string", "description": "mode=trash 时返回，可 Fs_ReadFile 读回" } }
        }))
        .example("--path tmp/old.log")
        .bind(Arc::new(RemoveFile))
}

pub fn remove_dir_definition() -> ActResult<CommandDef> {
    CommandBuilder::new("Fs_RemoveDir", "Delete ONE directory. Non-empty requires recursive=true; sandbox roots can never be removed; trash-first by default.", Capability::Write, "rd")
        .param(Param::string("path").alias("p").required().verify(Verify::PathLike).desc("目标目录（单个）"))
        .param(Param::boolean("recursive").alias("R").default(json!(false)).desc("非空目录必须显式传 true"))
        .output_done(json!({
            "type": "object", "required": ["ok", "command", "path", "deleted", "mode"],
            "properties": { "ok": { "type": "boolean" }, "command": { "const": "Fs_RemoveDir" },
                "path": { "type": "string" }, "deleted": { "type": "boolean" },
                "mode": { "enum": ["trash", "permanent"] }, "trash_path": { "type": "string" } }
        }))
        .example("--path build/dist --recursive")
        .bind(Arc::new(RemoveDir))
}
