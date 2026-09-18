//! Fs_RemoveFile / Fs_RemoveDir with trash-first semantics.

use std::sync::Arc;

use act_kernel::error::{ActError, ActResult};
use act_kernel::{Capability, CommandDef, CommandHandler, SandboxContext};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use crate::{trash::Trash, util};

#[derive(Deserialize)]
struct RemoveParams {
    paths: Vec<String>,
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
        if params.paths.is_empty() {
            return Err(ActError::invalid_params(
                "Fs_RemoveFile",
                "'paths' must not be empty",
            ));
        }
        let mut results = Vec::with_capacity(params.paths.len());
        for input in &params.paths {
            let item = match remove_one(ctx, input, false, false) {
                Ok(value) => value,
                Err(err) => util::err_item(input, &err),
            };
            results.push(item);
        }
        Ok(util::envelope("Fs_RemoveFile", results))
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
        if params.paths.is_empty() {
            return Err(ActError::invalid_params(
                "Fs_RemoveDir",
                "'paths' must not be empty",
            ));
        }
        let mut results = Vec::with_capacity(params.paths.len());
        for input in &params.paths {
            let item = match remove_one(ctx, input, true, params.recursive) {
                Ok(value) => value,
                Err(err) => util::err_item(input, &err),
            };
            results.push(item);
        }
        Ok(util::envelope("Fs_RemoveDir", results))
    }
}

fn remove_one(
    ctx: &SandboxContext,
    input: &str,
    dir: bool,
    recursive: bool,
) -> ActResult<serde_json::Value> {
    let (abs, root_idx) = ctx.resolve_path(input)?;
    util::ensure_not_root(ctx, &abs)?;

    let meta = std::fs::symlink_metadata(&abs)
        .map_err(|e| ActError::execution("remove", format!("cannot stat '{}': {}", input, e)))?;
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
        return Ok(util::ok_item(
            input,
            json!({ "deleted": true, "mode": "trash", "trash_path": rel }),
        ));
    }
    if dir {
        std::fs::remove_dir_all(&abs).map_err(|e| {
            ActError::execution("Fs_RemoveDir", format!("remove '{}': {}", input, e))
        })?;
    } else {
        std::fs::remove_file(&abs).map_err(|e| {
            ActError::execution("Fs_RemoveFile", format!("remove '{}': {}", input, e))
        })?;
    }
    Ok(util::ok_item(
        input,
        json!({ "deleted": true, "mode": "permanent" }),
    ))
}

pub fn remove_file_definition() -> ActResult<CommandDef> {
    CommandDef::new(
        "Fs_RemoveFile",
        "Delete files (batch). Default mode moves them to .act/trash/ (recoverable); configure fs.delete_mode for permanent deletion.",
        Capability::Write,
        json!({
            "type": "object",
            "properties": { "paths": { "type": "array", "items": { "type": "string" } } },
            "required": ["paths"]
        }),
        vec!["/paths/*".into()],
        vec![],
        Arc::new(RemoveFile),
    )
}

pub fn remove_dir_definition() -> ActResult<CommandDef> {
    CommandDef::new(
        "Fs_RemoveDir",
        "Delete directories (batch). Non-empty directories require recursive=true. Sandbox roots themselves can never be removed. Default mode is trash.",
        Capability::Write,
        json!({
            "type": "object",
            "properties": {
                "paths": { "type": "array", "items": { "type": "string" } },
                "recursive": { "type": "boolean", "description": "Required for non-empty directories" }
            },
            "required": ["paths"]
        }),
        vec!["/paths/*".into()],
        vec![],
        Arc::new(RemoveDir),
    )
}
