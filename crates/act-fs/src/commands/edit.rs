//! Fs_EditFile: exact string replacement in ONE file, preserving encoding.
//! Params are explicit paired arrays: `--old a --new x --old b --new y`
//! (no hidden string-splitting protocols inside values).

use std::sync::Arc;

use act_kernel::builder::{CommandBuilder, Param, Verify};
use act_kernel::error::{ActError, ActResult};
use act_kernel::{Capability, CommandDef, CommandHandler, SandboxContext};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use crate::{encoding, util};

#[derive(Deserialize)]
struct Params {
    path: String,
    old: Vec<String>,
    new: Vec<String>,
    #[serde(default)]
    replace_all: bool,
}

pub struct EditFile;

#[async_trait]
impl CommandHandler for EditFile {
    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: &SandboxContext,
    ) -> ActResult<serde_json::Value> {
        let params: Params = util::parse_params(params, "Fs_EditFile")?;
        if params.old.is_empty() || params.new.is_empty() {
            return Err(ActError::invalid_params(
                "Fs_EditFile",
                "--old and --new must each be given at least once",
            ));
        }
        if params.old.len() != params.new.len() {
            return Err(ActError::invalid_params(
                "Fs_EditFile",
                format!(
                    "--old given {} times but --new {} times; they must pair 1:1",
                    params.old.len(),
                    params.new.len()
                ),
            ));
        }
        let pairs: Vec<(&String, &String)> = params.old.iter().zip(params.new.iter()).collect();
        edit_one(
            ctx,
            &params.path,
            &pairs,
            params.replace_all,
            ctx.config.limits.max_read_bytes,
        )
    }
}

fn edit_one(
    ctx: &SandboxContext,
    input: &str,
    pairs: &[(&String, &String)],
    replace_all: bool,
    max_read: u64,
) -> ActResult<serde_json::Value> {
    let (abs, _) = ctx.resolve_path(input)?;
    let bytes = std::fs::read(&abs)
        .map_err(|e| ActError::execution("Fs_EditFile", format!("read '{}': {}", input, e)))?;
    if bytes.len() as u64 > max_read {
        return Err(ActError::LimitExceeded {
            reason: format!(
                "file too large for edit: {} bytes (cap {})",
                bytes.len(),
                max_read
            ),
        });
    }
    let decoded = encoding::decode(&bytes, &ctx.config.fs.fallback_encoding);
    if decoded.lossy {
        return Err(ActError::execution(
            "Fs_EditFile",
            format!(
                "'{}' cannot be decoded cleanly ({}); refusing to edit lossy content",
                input, decoded.encoding
            ),
        ));
    }

    let mut text = decoded.text;
    let mut replacements = 0usize;
    for (old, new) in pairs {
        if old == new {
            return Err(ActError::invalid_params(
                "Fs_EditFile",
                format!("--old and --new are identical: {:?}", old),
            ));
        }
        let count = text.matches(old.as_str()).count();
        if count == 0 {
            return Err(ActError::execution(
                "Fs_EditFile",
                format!("pattern not found in '{}': {:?}", input, old),
            ));
        }
        if count > 1 && !replace_all {
            return Err(ActError::execution(
                "Fs_EditFile",
                format!(
                    "pattern found {} times in '{}'; add --replace-all or use a longer unique pattern",
                    count, input
                ),
            ));
        }
        text = text.replace(old.as_str(), new.as_str());
        replacements += count;
    }

    let encoded = encoding::encode(&text, &decoded.encoding)?;
    util::atomic_write(&abs, &encoded)
        .map_err(|e| ActError::execution("Fs_EditFile", format!("write '{}': {}", input, e)))?;
    Ok(util::flat_ok(
        "Fs_EditFile",
        json!({ "path": input, "replacements": replacements, "encoding": decoded.encoding }),
    ))
}

pub fn definition() -> ActResult<CommandDef> {
    CommandBuilder::new(
        "Fs_EditFile",
        "Replace exact strings in ONE file. Paired --old/--new flags are repeatable (multiple edits apply atomically: all must match or nothing is written); ambiguous matches rejected unless --replace-all; original encoding preserved.",
        Capability::Write,
        "fe",
    )
    .param(Param::string("path").alias("p").required().verify(Verify::PathLike).desc("目标文件（单文件，可一次多处修改）"))
    .param(Param::array_of_string("old").alias("o").required().desc("被替换文本；与 --new 按出现顺序 1:1 配对，可重复"))
    .param(Param::array_of_string("new").alias("n").required().desc("替换文本；与 --old 配对，可重复"))
    .param(Param::boolean("replace_all").alias("all").default(json!(false)).desc("命中多次时全部替换（默认拒绝歧义匹配）"))
    .output_done(json!({
        "type": "object", "required": ["ok", "command", "path", "replacements"],
        "properties": { "ok": { "type": "boolean" }, "command": { "const": "Fs_EditFile" },
            "path": { "type": "string" }, "replacements": { "type": "integer" }, "encoding": { "type": "string" } }
    }))
    .output_fail("execution_failed", json!({
        "type": "object", "description": "pattern not found / 歧义匹配 / lossy 文件拒绝编辑",
        "properties": { "code": { "const": "execution_failed" }, "message": { "type": "string" } }
    }))
    .example("--path src/main.rs --old old_fn --new new_fn --old TODO --new DONE")
    .bind(Arc::new(EditFile))
}
