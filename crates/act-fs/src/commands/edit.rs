//! Fs_EditFile: exact string replacement in ONE file, preserving encoding.

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
    edits: Vec<EditPair>,
}

#[derive(Deserialize)]
struct EditPair {
    old: String,
    new: String,
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
        if params.edits.is_empty() {
            return Err(ActError::invalid_params(
                "Fs_EditFile",
                "'edits' must not be empty",
            ));
        }
        edit_one(
            ctx,
            &params.path,
            &params.edits,
            ctx.config.limits.max_read_bytes,
        )
    }
}

fn edit_one(
    ctx: &SandboxContext,
    input: &str,
    edits: &[EditPair],
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
    for pair in edits {
        if pair.old == pair.new {
            return Err(ActError::invalid_params(
                "Fs_EditFile",
                format!("old and new are identical: {:?}", pair.old),
            ));
        }
        let count = text.matches(&pair.old).count();
        if count == 0 {
            return Err(ActError::execution(
                "Fs_EditFile",
                format!("pattern not found in '{}': {:?}", input, pair.old),
            ));
        }
        if count > 1 && !pair.replace_all {
            return Err(ActError::execution(
                "Fs_EditFile",
                format!(
                    "pattern found {} times in '{}'; set replace_all=true or use a longer unique pattern",
                    count, input
                ),
            ));
        }
        text = text.replace(&pair.old, &pair.new);
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
    CommandBuilder::new("Fs_EditFile", "Replace exact strings in ONE file. Multiple edits apply atomically (all must match or nothing is written); ambiguous matches rejected unless replace_all; original encoding preserved.", Capability::Write, "fe")
        .param(Param::string("path").alias("p").required().verify(Verify::PathLike).desc("目标文件（单文件，可一次多处修改）"))
        .param(Param::array_of_object("edits").alias("edit").required().desc("CLI 糖：重复 --edit \"old=>new\""))
        .param(Param::boolean("replace_all").alias("all").default(json!(false)).desc("CLI 糖：应用到全部 edits"))
        .output_done(json!({
            "type": "object", "required": ["ok", "command", "path", "replacements"],
            "properties": { "ok": { "type": "boolean" }, "command": { "const": "Fs_EditFile" },
                "path": { "type": "string" }, "replacements": { "type": "integer" }, "encoding": { "type": "string" } }
        }))
        .example("--path src/main.rs --edit \"old_fn=>new_fn\" --edit \"TODO=>DONE\"")
        .bind(Arc::new(EditFile))
}
