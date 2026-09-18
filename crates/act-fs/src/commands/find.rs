//! Fs_FindFile: glob search (gitignore-aware) with file-name and relative-path
//! matching.

use std::sync::Arc;

use act_kernel::error::{ActError, ActResult};
use act_kernel::{
    builder::{CommandBuilder, Param, Verify},
    Capability, CommandDef, CommandHandler, SandboxContext,
};
use async_trait::async_trait;
use globset::{Glob, GlobSetBuilder};
use serde::Deserialize;
use serde_json::json;

use crate::util;

#[derive(Deserialize)]
struct Params {
    root: String,
    patterns: Vec<String>,
    #[serde(default)]
    max_depth: Option<u64>,
    #[serde(default)]
    include_hidden: bool,
    #[serde(default)]
    respect_gitignore: Option<bool>,
    #[serde(default)]
    max_results: Option<usize>,
}

pub struct FindFile;

#[async_trait]
impl CommandHandler for FindFile {
    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: &SandboxContext,
    ) -> ActResult<serde_json::Value> {
        let params: Params = util::parse_params(params, "Fs_FindFile")?;
        if params.patterns.is_empty() {
            return Err(ActError::invalid_params(
                "Fs_FindFile",
                "'patterns' must not be empty",
            ));
        }
        let (abs, _) = ctx.resolve_path(&params.root)?;
        if !abs.is_dir() {
            return Err(ActError::invalid_params(
                "Fs_FindFile",
                format!("root '{}' is not a directory", params.root),
            ));
        }

        let mut rel_set = GlobSetBuilder::new();
        let mut name_set = GlobSetBuilder::new();
        for pattern in &params.patterns {
            let glob = Glob::new(pattern).map_err(|e| {
                ActError::invalid_params("Fs_FindFile", format!("bad glob '{pattern}': {e}"))
            })?;
            rel_set.add(glob.clone());
            name_set.add(glob);
        }
        let rel_matcher = rel_set
            .build()
            .map_err(|e| ActError::invalid_params("Fs_FindFile", e.to_string()))?;
        let name_matcher = name_set
            .build()
            .map_err(|e| ActError::invalid_params("Fs_FindFile", e.to_string()))?;

        let cap = params
            .max_results
            .unwrap_or(ctx.config.limits.max_find_results);
        let max_depth = params
            .max_depth
            .unwrap_or(ctx.config.limits.max_depth as u64)
            .min(ctx.config.limits.max_depth as u64);
        let respect_git = params.respect_gitignore.unwrap_or(true);

        let mut walker = ignore::WalkBuilder::new(&abs);
        walker
            .hidden(!params.include_hidden)
            .git_ignore(respect_git)
            .git_global(false)
            .git_exclude(respect_git)
            .ignore(respect_git)
            .parents(respect_git)
            .max_depth(Some(max_depth as usize));

        let mut matches = Vec::new();
        let mut truncated = false;
        for entry in walker.build().filter_map(|e| e.ok()) {
            let path = entry.path();
            let rel = path.strip_prefix(&abs).unwrap_or(path);
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            let is_match = if rel_str.is_empty() {
                false
            } else {
                rel_matcher.is_match(&rel_str)
                    || path
                        .file_name()
                        .map(|n| name_matcher.is_match(n.to_string_lossy().as_ref()))
                        .unwrap_or(false)
            };
            if !is_match {
                continue;
            }
            if matches.len() >= cap {
                truncated = true;
                break;
            }
            let meta = entry.metadata().ok();
            matches.push(json!({
                "path": util::join_user_path(&params.root, rel),
                "is_dir": path.is_dir(),
                "size": meta.as_ref().map(|m| m.len()).unwrap_or(0),
            }));
        }

        Ok(json!({
            "ok": true,
            "command": "Fs_FindFile",
            "root": params.root,
            "matches": matches,
            "count": matches.len(),
            "truncated": truncated,
        }))
    }
}

pub fn definition() -> ActResult<CommandDef> {
    CommandBuilder::new("Fs_FindFile", "Find files by glob patterns under a root (patterns are OR-ed; gitignore-aware by default; matches relative path or file name).", Capability::Read, "find")
        .param(Param::string("root").alias("r").required().verify(Verify::PathLike))
        .param(Param::array_of_string("patterns").alias("pattern").required().desc("globset 语法；同时匹配文件名与相对路径"))
        .param(Param::integer("max_depth").alias("d").alias("depth").min(1.0).max(64.0))
        .param(Param::boolean("include_hidden").alias("h").alias("hidden").default(json!(false)))
        .param(Param::boolean("respect_gitignore").alias("no-gitignore").default(json!(true)).desc("CLI 否定形：--no-gitignore"))
        .param(Param::integer("max_results").alias("m").alias("max").min(1.0).max(500.0))
        .output_done(json!({
            "type": "object", "required": ["ok", "command", "root", "matches", "count"],
            "properties": { "ok": { "type": "boolean" }, "command": { "const": "Fs_FindFile" },
                "root": { "type": "string" },
                "matches": { "type": "array", "items": { "type": "object", "properties": {
                    "path": { "type": "string", "description": "root+相对路径，可直接回传其他命令" },
                    "is_dir": { "type": "boolean" }, "size": { "type": "integer" } } } },
                "count": { "type": "integer" }, "truncated": { "type": "boolean" } }
        }))
        .example("--root crates --pattern *.rs --pattern *.toml --max 50")
        .bind(Arc::new(FindFile))
}
