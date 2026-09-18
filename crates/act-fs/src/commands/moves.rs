//! Fs_MoveFile / Fs_MoveDir / Fs_CopyFile / Fs_CopyDir (single from→to pair).

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
struct MoveParams {
    from: String,
    to: String,
    #[serde(default)]
    overwrite: bool,
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
        run(ctx, params, Kind::MoveFile).await
    }
}

#[async_trait]
impl CommandHandler for MoveDir {
    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: &SandboxContext,
    ) -> ActResult<serde_json::Value> {
        run(ctx, params, Kind::MoveDir).await
    }
}

#[async_trait]
impl CommandHandler for CopyFile {
    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: &SandboxContext,
    ) -> ActResult<serde_json::Value> {
        run(ctx, params, Kind::CopyFile).await
    }
}

#[async_trait]
impl CommandHandler for CopyDir {
    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: &SandboxContext,
    ) -> ActResult<serde_json::Value> {
        run(ctx, params, Kind::CopyDir).await
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
    kind: Kind,
) -> ActResult<serde_json::Value> {
    let params: MoveParams = util::parse_params(params, command_of(kind))?;
    one(ctx, &params.from, &params.to, params.overwrite, kind)
}

fn command_of(kind: Kind) -> &'static str {
    match kind {
        Kind::MoveFile => "Fs_MoveFile",
        Kind::MoveDir => "Fs_MoveDir",
        Kind::CopyFile => "Fs_CopyFile",
        Kind::CopyDir => "Fs_CopyDir",
    }
}

fn one(
    ctx: &SandboxContext,
    from_input: &str,
    to_input: &str,
    overwrite: bool,
    kind: Kind,
) -> ActResult<serde_json::Value> {
    let command = command_of(kind);
    let (from, _) = ctx.resolve_path(from_input)?;
    let (to, to_root) = ctx.resolve_path(to_input)?;
    util::ensure_not_root(ctx, &from)?;
    util::ensure_not_root(ctx, &to)?;

    if act_kernel::guard::path_eq(&from, &to) {
        return Err(ActError::invalid_params(
            command,
            "from and to are the same path",
        ));
    }
    if matches!(kind, Kind::MoveDir | Kind::CopyDir)
        && act_kernel::guard::path_guard::strip_prefix_ci(&to, &from).is_some()
    {
        return Err(ActError::invalid_params(
            command,
            "destination is inside the source directory",
        ));
    }

    match kind {
        Kind::MoveFile | Kind::CopyFile => {
            if !from.is_file() {
                return Err(ActError::invalid_params(
                    command,
                    format!("'{}' is not a file", from_input),
                ));
            }
        }
        Kind::MoveDir | Kind::CopyDir => {
            if !from.is_dir() {
                return Err(ActError::invalid_params(
                    command,
                    format!("'{}' is not a directory", from_input),
                ));
            }
        }
    }

    if to.exists() || std::fs::symlink_metadata(&to).is_ok() {
        if !overwrite {
            return Err(ActError::execution(
                command,
                format!("destination exists: '{}'", to_input),
            ));
        }
        Trash::stash(ctx, &to, to_root)?;
    }

    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(ActError::Io)?;
    }

    let moved = matches!(kind, Kind::MoveFile | Kind::MoveDir);
    if moved {
        if let Err(e) = std::fs::rename(&from, &to) {
            if e.raw_os_error() == Some(18) || e.kind() == std::io::ErrorKind::CrossesDevices {
                crate::trash::copy_recursive(&from, &to)?;
                if from.is_dir() {
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
    Ok(util::flat_ok(
        command,
        json!({
            "from": from_input,
            "to": to_input,
            "op": if moved { "moved" } else { "copied" },
        }),
    ))
}

pub fn move_file_definition() -> ActResult<CommandDef> {
    CommandBuilder::new("Fs_MoveFile", "Move/rename ONE file. Cross-volume moves fall back to copy+delete.", Capability::Write, "mf")
        .param(Param::string("from").alias("f").required().verify(Verify::PathLike))
        .param(Param::string("to").alias("t").required().verify(Verify::PathLike))
        .param(Param::boolean("overwrite").alias("w").default(json!(false)).desc("true 时旧目标先进 trash"))
        .output_done(json!({
            "type": "object", "required": ["ok", "command", "from", "to", "op"],
            "properties": { "ok": { "type": "boolean" }, "command": { "const": "Fs_MoveFile" },
                "from": { "type": "string" }, "to": { "type": "string" }, "op": { "enum": ["moved", "copied"] } }
        }))
        .example("--from src/old.rs --to src/new.rs")
        .bind(Arc::new(MoveFile))
}

pub fn move_dir_definition() -> ActResult<CommandDef> {
    CommandBuilder::new("Fs_MoveDir", "Move/rename ONE directory. Nesting the destination inside its source is rejected.", Capability::Write, "md")
        .param(Param::string("from").alias("f").required().verify(Verify::PathLike))
        .param(Param::string("to").alias("t").required().verify(Verify::PathLike))
        .param(Param::boolean("overwrite").alias("w").default(json!(false)))
        .output_done(json!({
            "type": "object", "required": ["ok", "command", "from", "to", "op"],
            "properties": { "ok": { "type": "boolean" }, "command": { "const": "Fs_MoveDir" },
                "from": { "type": "string" }, "to": { "type": "string" }, "op": { "enum": ["moved", "copied"] } }
        }))
        .example("--from docs/old --to docs/new")
        .bind(Arc::new(MoveDir))
}

pub fn copy_file_definition() -> ActResult<CommandDef> {
    CommandBuilder::new("Fs_CopyFile", "Copy ONE file; parent directories are created automatically.", Capability::Write, "cf")
        .param(Param::string("from").alias("f").required().verify(Verify::PathLike))
        .param(Param::string("to").alias("t").required().verify(Verify::PathLike))
        .param(Param::boolean("overwrite").alias("w").default(json!(false)))
        .output_done(json!({
            "type": "object", "required": ["ok", "command", "from", "to", "op"],
            "properties": { "ok": { "type": "boolean" }, "command": { "const": "Fs_CopyFile" },
                "from": { "type": "string" }, "to": { "type": "string" }, "op": { "enum": ["moved", "copied"] } }
        }))
        .example("--from config/default.toml --to config/local.toml")
        .bind(Arc::new(CopyFile))
}

pub fn copy_dir_definition() -> ActResult<CommandDef> {
    CommandBuilder::new("Fs_CopyDir", "Copy ONE directory recursively.", Capability::Write, "cp")
        .param(Param::string("from").alias("f").required().verify(Verify::PathLike))
        .param(Param::string("to").alias("t").required().verify(Verify::PathLike))
        .param(Param::boolean("overwrite").alias("w").default(json!(false)))
        .output_done(json!({
            "type": "object", "required": ["ok", "command", "from", "to", "op"],
            "properties": { "ok": { "type": "boolean" }, "command": { "const": "Fs_CopyDir" },
                "from": { "type": "string" }, "to": { "type": "string" }, "op": { "enum": ["moved", "copied"] } }
        }))
        .example("--from templates/react --to projects/my-app")
        .bind(Arc::new(CopyDir))
}
