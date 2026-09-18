//! OutputPolicy: inline results, overflow files and summary envelopes.
//!
//! When a serialized result exceeds `max_inline_bytes` and compression is
//! enabled, the full result is written under the primary root's overflow
//! directory and replaced by `{truncated, summary, full_content_path}`.

use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::config::CompressionConfig;
use crate::context::SandboxContext;
use crate::error::{ActError, ActResult};

pub struct OutputPolicy;

impl OutputPolicy {
    pub fn new() -> Self {
        Self
    }

    /// Apply the policy to a command result.
    pub async fn apply(
        &self,
        command: &str,
        result: Value,
        sandbox: &Arc<SandboxContext>,
    ) -> ActResult<Value> {
        let out_cfg = &sandbox.config.output;
        let serialized = serde_json::to_vec(&result)
            .map_err(|e| ActError::Other(format!("serialize result: {e}")))?;
        if serialized.len() <= out_cfg.max_inline_bytes {
            return Ok(result);
        }

        let original_bytes = serialized.len();
        if !out_cfg.compression.enabled {
            // Hard truncate with a preview.
            let preview = String::from_utf8_lossy(
                &serialized[..out_cfg.max_inline_bytes.min(serialized.len())],
            )
            .to_string();
            return Ok(serde_json::json!({
                "truncated": true,
                "compressed": false,
                "command": command,
                "original_bytes": original_bytes,
                "preview": preview,
            }));
        }

        let overflow_rel = self.write_overflow(command, &result, sandbox)?;
        let text = crate::summarize::collect_text(&result);
        let summary = crate::summarize::summarize(&text, &out_cfg.compression).await?;

        Ok(serde_json::json!({
            "truncated": true,
            "compressed": true,
            "command": command,
            "original_bytes": original_bytes,
            "summary": summary,
            "full_content_path": overflow_rel,
        }))
    }

    fn write_overflow(
        &self,
        command: &str,
        result: &Value,
        sandbox: &Arc<SandboxContext>,
    ) -> ActResult<String> {
        let out_cfg = &sandbox.config.output;
        let dir: PathBuf = sandbox
            .primary_root()
            .join(&out_cfg.overflow_dir)
            .join(command);
        std::fs::create_dir_all(&dir).map_err(|e| ActError::Io(e))?;

        self.purge_expired(&dir, out_cfg.overflow_retention_days);

        let pretty = serde_json::to_vec_pretty(result)
            .map_err(|e| ActError::Other(format!("serialize overflow: {e}")))?;
        let hash = hex::encode(&Sha256::digest(&pretty)[..3]);
        let ts = Utc::now().format("%Y%m%d-%H%M%S%3f");
        let file = dir.join(format!("{}-{}.json", ts, hash));
        std::fs::write(&file, &pretty).map_err(ActError::Io)?;

        sandbox
            .rel_to_root(&file)
            .map(|p| p.to_string_lossy().replace('\\', "/"))
            .ok_or_else(|| ActError::Other("overflow file escaped sandbox root".into()))
    }

    fn purge_expired(&self, dir: &std::path::Path, retention_days: u32) {
        if retention_days == 0 {
            return;
        }
        let cutoff = std::time::SystemTime::now()
            - std::time::Duration::from_secs(retention_days as u64 * 24 * 3600);
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                if let Ok(meta) = entry.metadata() {
                    if let Ok(modified) = meta.modified() {
                        if modified < cutoff {
                            let _ = std::fs::remove_file(entry.path());
                        }
                    }
                }
            }
        }
    }

    /// Configuration accessor used by tests and the CLI.
    pub fn compression(cfg: &crate::config::ActConfig) -> &CompressionConfig {
        &cfg.output.compression
    }
}

impl Default for OutputPolicy {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn sandbox_in(dir: &std::path::Path) -> Arc<SandboxContext> {
        let cfg = crate::config::ActConfig {
            roots: vec![dir.to_path_buf()],
            ..Default::default()
        };
        crate::context::SandboxContext::new(
            std::sync::Arc::new(cfg),
            std::sync::Arc::new(
                crate::guard::path_guard::PathGuard::new(vec![dir.to_path_buf()]).unwrap(),
            ),
            crate::guard::url_guard::UrlGuard::new(Default::default()),
            crate::guard::protect_guard::ProtectGuard::from_config(&[], false),
        )
        .into()
    }

    #[tokio::test]
    async fn small_result_inline() {
        let tmp = tempfile::tempdir().unwrap();
        let sb = sandbox_in(tmp.path()).await;
        let policy = OutputPolicy::new();
        let result = serde_json::json!({"ok": true, "data": "small"});
        let out = policy
            .apply("Fs_ReadFile", result.clone(), &sb)
            .await
            .unwrap();
        assert_eq!(out, result);
    }

    #[tokio::test]
    async fn oversized_result_overflowed() {
        let tmp = tempfile::tempdir().unwrap();
        let sb = sandbox_in(tmp.path()).await;
        let policy = OutputPolicy::new();
        let big_text = "agent core tools kernel. ".repeat(4000);
        let result = serde_json::json!({"ok": true, "content": big_text});
        let out = policy.apply("Fs_ReadFile", result, &sb).await.unwrap();
        assert_eq!(out["truncated"], serde_json::json!(true));
        assert!(out["summary"].as_str().unwrap_or("").len() > 10);
        let rel = out["full_content_path"].as_str().unwrap();
        assert!(rel.starts_with(".act/overflow/Fs_ReadFile/"));
        let full = tmp.path().join(rel);
        assert!(full.is_file(), "overflow file must exist");
        let body: Value = serde_json::from_str(&std::fs::read_to_string(&full).unwrap()).unwrap();
        assert_eq!(body["ok"], serde_json::json!(true));
    }

    #[tokio::test]
    async fn compression_disabled_truncates() {
        let tmp = tempfile::tempdir().unwrap();
        let mut cfg = crate::config::ActConfig {
            roots: vec![tmp.path().to_path_buf()],
            ..Default::default()
        };
        cfg.output.compression.enabled = false;
        cfg.output.max_inline_bytes = 128;
        let sb = Arc::new(crate::context::SandboxContext::new(
            Arc::new(cfg),
            Arc::new(
                crate::guard::path_guard::PathGuard::new(vec![tmp.path().to_path_buf()]).unwrap(),
            ),
            crate::guard::url_guard::UrlGuard::new(Default::default()),
            crate::guard::protect_guard::ProtectGuard::from_config(&[], false),
        ));
        let policy = OutputPolicy::new();
        let result = serde_json::json!({"content": "x".repeat(1000)});
        let out = policy.apply("Fs_ReadFile", result, &sb).await.unwrap();
        assert_eq!(out["truncated"], serde_json::json!(true));
        assert!(out.get("full_content_path").is_none());
    }
}
