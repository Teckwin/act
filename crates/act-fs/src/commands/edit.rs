//! Fs_EditFile: exact string replacement preserving the file's encoding.

use std::sync::Arc;

use act_kernel::error::{ActError, ActResult};
use act_kernel::{Capability, CommandDef, CommandHandler, SandboxContext};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::json;

use crate::{encoding, util};

#[derive(Deserialize)]
struct Params {
    files: Vec<EditSpec>,
}

#[derive(Deserialize)]
struct EditSpec {
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
        if params.files.is_empty() {
            return Err(ActError::invalid_params(
                "Fs_EditFile",
                "'files' must not be empty",
            ));
        }
        let max_read = ctx.config.limits.max_read_bytes;
        let mut results = Vec::with_capacity(params.files.len());
        for spec in &params.files {
            let item = match edit_one(ctx, spec, max_read) {
                Ok(value) => value,
                Err(err) => util::err_item(&spec.path, &err),
            };
            results.push(item);
        }
        Ok(util::envelope("Fs_EditFile", results))
    }
}

fn edit_one(ctx: &SandboxContext, spec: &EditSpec, max_read: u64) -> ActResult<serde_json::Value> {
    if spec.edits.is_empty() {
        return Err(ActError::invalid_params(
            "Fs_EditFile",
            "'edits' must not be empty",
        ));
    }
    let (abs, _) = ctx.resolve_path(&spec.path)?;
    let bytes = std::fs::read(&abs)
        .map_err(|e| ActError::execution("Fs_EditFile", format!("read '{}': {}", spec.path, e)))?;
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
                spec.path, decoded.encoding
            ),
        ));
    }

    let mut text = decoded.text;
    let mut replacements = 0usize;
    for pair in &spec.edits {
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
                format!("pattern not found in '{}': {:?}", spec.path, pair.old),
            ));
        }
        if count > 1 && !pair.replace_all {
            return Err(ActError::execution(
                "Fs_EditFile",
                format!(
                    "pattern found {} times in '{}'; set replace_all=true or use a longer unique pattern",
                    count, spec.path
                ),
            ));
        }
        text = text.replace(&pair.old, &pair.new);
        replacements += count;
    }

    // Re-encode with the detected encoding to preserve the original format.
    let encoded = encoding::encode(&text, &decoded.encoding)?;
    util::atomic_write(&abs, &encoded)
        .map_err(|e| ActError::execution("Fs_EditFile", format!("write '{}': {}", spec.path, e)))?;
    Ok(util::ok_item(
        &spec.path,
        json!({ "replacements": replacements, "encoding": decoded.encoding }),
    ))
}

pub fn definition() -> ActResult<CommandDef> {
    CommandDef::new(
        "Fs_EditFile",
        "Replace exact strings in files (batch). Each edit is {old,new,replace_all}; ambiguous matches are rejected; original encoding is preserved.",
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
                            "edits": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "old": { "type": "string" },
                                        "new": { "type": "string" },
                                        "replace_all": { "type": "boolean" }
                                    },
                                    "required": ["old", "new"]
                                }
                            }
                        },
                        "required": ["path", "edits"]
                    }
                }
            },
            "required": ["files"]
        }),
        vec!["/files/*/path".into()],
        vec![],
        Arc::new(EditFile),
    )
}
