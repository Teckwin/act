//! Fs_MoveFile / Fs_MoveDir / Fs_CopyFile / Fs_CopyDir.

use std::sync::Arc;

use act_kernel::error::{ActError, ActResult};
use act_kernel::guard::path_eq;
use act_kernel::{Capability, CommandDef, CommandHandler, SandboxContext};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use crate::{trash::Trash, util};

#[derive(Deserialize)]
struct MoveParams {
    moves: Vec<MovePair>,
    #[serde(default)]
    overwrite: bool,
}

#[derive(Deserialize)]
struct MovePair {
    from: String,
    to: String,
}

pub struct MoveFile;
pub struct MoveDir;
pub struct CopyFile;
pub struct CopyDir;

#[async_trait]
impl CommandHandler for MoveFile {
    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: &SandboxContext,
    ) -> ActResult<serde_json::Value> {
        run(ctx, params, "Fs_MoveFile", Kind::MoveFile).await
    }
}

#[async_trait]
impl CommandHandler for MoveDir {
    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: &SandboxContext,
    ) -> ActResult<serde_json::Value> {
        run(ctx, params, "Fs_MoveDir", Kind::MoveDir).await
    }
}

#[async_trait]
impl CommandHandler for CopyFile {
    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: &SandboxContext,
    ) -> ActResult<serde_json::Value> {
        run(ctx, params, "Fs_CopyFile", Kind::CopyFile).await
    }
}

#[async_trait]
impl CommandHandler for CopyDir {
    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: &SandboxContext,
    ) -> ActResult<serde_json::Value> {
        run(ctx, params, "Fs_CopyDir", Kind::CopyDir).await
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    MoveFile,
    MoveDir,
    CopyFile,
    CopyDir,
}

async fn run(
    ctx: &SandboxContext,
    params: serde_json::Value,
    command: &str,
    kind: Kind,
) -> ActResult<serde_json::Value> {
    let params: MoveParams = util::parse_params(params, command)?;
    if params.moves.is_empty() {
        return Err(ActError::invalid_params(
            command,
            "'moves' must not be empty",
        ));
    }
    let mut results = Vec::with_capacity(params.moves.len());
    for pair in &params.moves {
        let item = match one(ctx, pair, params.overwrite, kind) {
            Ok(value) => value,
            Err(err) => {
                let mut item = util::err_item(&pair.from, &err);
                item["to"] = json!(pair.to);
                item
            }
        };
        results.push(item);
    }
    Ok(util::envelope(command, results))
}

fn one(
    ctx: &SandboxContext,
    pair: &MovePair,
    overwrite: bool,
    kind: Kind,
) -> ActResult<serde_json::Value> {
    let command = match kind {
        Kind::MoveFile => "Fs_MoveFile",
        Kind::MoveDir => "Fs_MoveDir",
        Kind::CopyFile => "Fs_CopyFile",
        Kind::CopyDir => "Fs_CopyDir",
    };
    let (from, from_root) = ctx.resolve_path(&pair.from)?;
    let (to, to_root) = ctx.resolve_path(&pair.to)?;
    util::ensure_not_root(ctx, &from)?;
    util::ensure_not_root(ctx, &to)?;

    if path_eq(&from, &to) {
        return Err(ActError::invalid_params(
            command,
            "from and to are the same path",
        ));
    }
    // Reject nesting the destination inside the source (dir cases).
    if matches!(kind, Kind::MoveDir | Kind::CopyDir)
        && act_kernel::guard::path_guard::strip_prefix_ci(&to, &from).is_some()
    {
        return Err(ActError::invalid_params(
            command,
            "destination is inside the source directory",
        ));
    }

    let from_is_dir = from.is_dir();
    match kind {
        Kind::MoveFile => {
            if !from.is_file() {
                return Err(ActError::invalid_params(
                    command,
                    format!("'{}' is not a file", pair.from),
                ));
            }
        }
        Kind::CopyFile => {
            if !from.is_file() {
                return Err(ActError::invalid_params(
                    command,
                    format!("'{}' is not a file", pair.from),
                ));
            }
        }
        Kind::MoveDir | Kind::CopyDir => {
            if !from_is_dir {
                return Err(ActError::invalid_params(
                    command,
                    format!("'{}' is not a directory", pair.from),
                ));
            }
        }
    }

    // Destination handling.
    if to.exists() || std::fs::symlink_metadata(&to).is_ok() {
        if !overwrite {
            return Err(ActError::execution(
                command,
                format!("destination exists: '{}'", pair.to),
            ));
        }
        // Stash the overwritten destination in the trash for safety.
        let dest_root = if to.exists() { to_root } else { to_root };
        Trash::stash(ctx, &to, dest_root)?;
    }

    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(ActError::Io)?;
    }

    let moved = matches!(kind, Kind::MoveFile | Kind::MoveDir);
    if moved {
        if let Err(e) = std::fs::rename(&from, &to) {
            if e.raw_os_error() == Some(18) || e.kind() == std::io::ErrorKind::CrossesDevices {
                crate::trash::copy_recursive(&from, &to)?;
                if from_is_dir {
                    std::fs::remove_dir_all(&from).map_err(ActError::Io)?;
                } else {
                    std::fs::remove_file(&from).map_err(ActError::Io)?;
                }
            } else {
                return Err(ActError::Io(e));
            }
        }
    } else {
        crate::trash::copy_recursive(&from, &to)?;
    }
    let _ = from_root;
    Ok(util::ok_item(
        &pair.from,
        json!({ "to": pair.to, "op": if moved { "moved" } else { "copied" } }),
    ))
}

fn move_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "moves": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "from": { "type": "string" },
                        "to": { "type": "string" }
                    },
                    "required": ["from", "to"]
                }
            },
            "overwrite": { "type": "boolean", "description": "Overwrite existing destinations (old target goes to trash)" }
        },
        "required": ["moves"]
    })
}

pub fn move_file_definition() -> ActResult<CommandDef> {
    CommandDef::new(
        "Fs_MoveFile",
        "Move/rename files (batch). Cross-volume moves fall back to copy+delete.",
        Capability::Write,
        move_schema(),
        vec!["/moves/*/from".into(), "/moves/*/to".into()],
        vec![],
        Arc::new(MoveFile),
    )
}

pub fn move_dir_definition() -> ActResult<CommandDef> {
    CommandDef::new(
        "Fs_MoveDir",
        "Move/rename directories (batch). Nesting a destination inside its source is rejected.",
        Capability::Write,
        move_schema(),
        vec!["/moves/*/from".into(), "/moves/*/to".into()],
        vec![],
        Arc::new(MoveDir),
    )
}

pub fn copy_file_definition() -> ActResult<CommandDef> {
    CommandDef::new(
        "Fs_CopyFile",
        "Copy files (batch); parent directories are created automatically.",
        Capability::Write,
        move_schema(),
        vec!["/moves/*/from".into(), "/moves/*/to".into()],
        vec![],
        Arc::new(CopyFile),
    )
}

pub fn copy_dir_definition() -> ActResult<CommandDef> {
    CommandDef::new(
        "Fs_CopyDir",
        "Copy directories recursively (batch).",
        Capability::Write,
        move_schema(),
        vec!["/moves/*/from".into(), "/moves/*/to".into()],
        vec![],
        Arc::new(CopyDir),
    )
}
