//! Fs_GrepFile: regex content search, decoding each file with its own
//! encoding before matching (no more mojibake misses on Chinese content).

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
    root: String,
    pattern: String,
    #[serde(default)]
    globs: Vec<String>,
    #[serde(default)]
    include_hidden: bool,
    #[serde(default)]
    respect_gitignore: Option<bool>,
    #[serde(default)]
    max_results: Option<usize>,
    #[serde(default)]
    context_lines: u64,
}

pub struct GrepFile;

#[async_trait]
impl CommandHandler for GrepFile {
    async fn execute(
        &self,
        params: serde_json::Value,
        ctx: &SandboxContext,
    ) -> ActResult<serde_json::Value> {
        let params: Params = util::parse_params(params, "Fs_GrepFile")?;
        let re = regex::Regex::new(&params.pattern)
            .map_err(|e| ActError::invalid_params("Fs_GrepFile", format!("bad regex: {e}")))?;
        let (abs, _) = ctx.resolve_path(&params.root)?;
        if !abs.is_dir() {
            return Err(ActError::invalid_params(
                "Fs_GrepFile",
                format!("root '{}' is not a directory", params.root),
            ));
        }

        let mut glob_matcher = None;
        if !params.globs.is_empty() {
            let mut builder = globset::GlobSetBuilder::new();
            for g in &params.globs {
                let glob = globset::Glob::new(g).map_err(|e| {
                    ActError::invalid_params("Fs_GrepFile", format!("bad glob '{g}': {e}"))
                })?;
                builder.add(glob);
            }
            glob_matcher = Some(
                builder
                    .build()
                    .map_err(|e| ActError::invalid_params("Fs_GrepFile", e.to_string()))?,
            );
        }

        let cap = params
            .max_results
            .unwrap_or(ctx.config.limits.max_grep_results);
        let max_read = ctx.config.limits.max_read_bytes;
        let context = params.context_lines.min(10) as usize;
        let respect_git = params.respect_gitignore.unwrap_or(true);

        let mut walker = ignore::WalkBuilder::new(&abs);
        walker
            .hidden(!params.include_hidden)
            .git_ignore(respect_git)
            .git_global(false)
            .git_exclude(respect_git)
            .ignore(respect_git)
            .parents(respect_git)
            .max_depth(Some(ctx.config.limits.max_depth));

        let mut files = Vec::new();
        let mut total_lines = 0usize;
        let mut truncated = false;

        for entry in walker.build().filter_map(|e| e.ok()) {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let rel = path.strip_prefix(&abs).unwrap_or(path);
            let rel_str = rel.to_string_lossy().replace('\\', "/");
            if let Some(matcher) = &glob_matcher {
                let hit = matcher.is_match(&rel_str)
                    || path
                        .file_name()
                        .map(|n| matcher.is_match(n.to_string_lossy().as_ref()))
                        .unwrap_or(false);
                if !hit {
                    continue;
                }
            }
            let Ok(bytes) = std::fs::read(path) else {
                continue;
            };
            if bytes.len() as u64 > max_read || bytes.len() > 16 * 1024 * 1024 {
                continue;
            }
            let decoded = encoding::decode(&bytes, &ctx.config.fs.fallback_encoding);
            if decoded.lossy {
                // Binary-ish file; skip.
                continue;
            }

            let lines: Vec<&str> = decoded.text.lines().collect();
            let mut match_lines: Vec<serde_json::Value> = Vec::new();
            let mut matched_any = false;
            let mut n = 0usize;
            while n < lines.len() {
                if re.is_match(lines[n]) {
                    matched_any = true;
                    if total_lines >= cap {
                        truncated = true;
                        break;
                    }
                    let lo = n.saturating_sub(context);
                    let hi = (n + context).min(lines.len() - 1);
                    let block: Vec<String> = (lo..=hi)
                        .map(|i| {
                            let marker = if i == n { ">" } else { " " };
                            format!("{}{:>5} | {}", marker, i + 1, lines[i])
                        })
                        .collect();
                    match_lines.push(json!({
                        "line": n + 1,
                        "text": lines[n],
                        "context": if context > 0 { json!(block.join("\n")) } else { serde_json::Value::Null },
                    }));
                    total_lines += 1;
                    n = hi + 1;
                } else {
                    n += 1;
                }
            }
            if matched_any {
                files.push(json!({
                    "path": util::join_user_path(&params.root, rel),
                    "encoding": decoded.encoding,
                    "match_count": match_lines.len(),
                    "matches": match_lines,
                }));
            }
            if truncated {
                break;
            }
        }

        Ok(json!({
            "ok": true,
            "command": "Fs_GrepFile",
            "root": params.root,
            "pattern": params.pattern,
            "files": files,
            "file_count": files.len(),
            "match_lines": total_lines,
            "truncated": truncated,
        }))
    }
}

pub fn definition() -> ActResult<CommandDef> {
    CommandBuilder::new("Fs_GrepFile", "Regex content search under a root. Each file is decoded (UTF-8/BOM/UTF-16/fallback) before matching; gitignore-aware; optional glob filter and context lines.", Capability::Read, "grep")
        .param(Param::string("root").alias("r").required().verify(Verify::PathLike))
        .param(Param::string("pattern").alias("e").required().desc("Rust regex（不支持 lookahead/backreference）"))
        .param(Param::array_of_string("globs").alias("g").alias("glob").desc("文件过滤 glob，可重复/逗号分隔"))
        .param(Param::boolean("include_hidden").alias("h").alias("hidden").default(json!(false)))
        .param(Param::boolean("respect_gitignore").alias("no-gitignore").default(json!(true)).desc("CLI 否定形：--no-gitignore"))
        .param(Param::integer("max_results").alias("m").alias("max").min(1.0).max(500.0).desc("匹配行总数上限"))
        .param(Param::integer("context_lines").alias("c").alias("context").min(0.0).max(10.0).default(json!(0)))
        .output_done(json!({
            "type": "object", "required": ["ok", "command", "root", "pattern", "files", "file_count"],
            "properties": { "ok": { "type": "boolean" }, "command": { "const": "Fs_GrepFile" },
                "root": { "type": "string" }, "pattern": { "type": "string" },
                "files": { "type": "array", "items": { "type": "object", "properties": {
                    "path": { "type": "string" }, "encoding": { "type": "string" }, "match_count": { "type": "integer" },
                    "matches": { "type": "array", "items": { "type": "object", "properties": {
                        "line": { "type": "integer", "description": "1-based 行号" }, "text": { "type": "string" },
                        "context": { "type": "string", "description": "context_lines>0 时非 null，> 标记命中行" } } } } } } },
                "file_count": { "type": "integer" }, "match_lines": { "type": "integer" }, "truncated": { "type": "boolean" } }
        }))
        .example("--root src --pattern \"TODO|FIXME\" --glob *.rs --context 2")
        .bind(Arc::new(GrepFile))
}
