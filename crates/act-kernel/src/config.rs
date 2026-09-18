//! Configuration: defaults <- config file <- environment overrides.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{ActError, ActResult};

pub const DEFAULT_PROTECTED_GLOBS: &[&str] = &[
    "**/.git/**",
    "**/.git",
    ".env",
    ".env.*",
    "**/.env",
    "**/.env.*",
    "**/*.pem",
    "**/*.key",
    "**/id_rsa*",
    "**/id_ed25519*",
    "**/.act/**",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum DeleteMode {
    /// Move deleted entries into `.act/trash/<timestamp>/` (default, safer).
    #[default]
    Trash,
    /// Permanently delete.
    Permanent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SummaryAlgorithm {
    /// Local extractive summarizer, zero external dependencies (default).
    #[default]
    Extractive,
    /// OpenAI-compatible chat API; falls back to extractive on failure.
    Llm,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "snake_case")]
pub struct ModeCapabilities {
    pub read: bool,
    pub write: bool,
    pub net: bool,
}

impl Default for ModeCapabilities {
    fn default() -> Self {
        Self {
            read: true,
            write: true,
            net: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Capabilities {
    pub cli: ModeCapabilities,
    pub mcp: ModeCapabilities,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct Limits {
    /// Max bytes returned from a single file read.
    pub max_read_bytes: u64,
    /// Max bytes accepted for a single file write.
    pub max_write_bytes: u64,
    /// Max number of path/url items per command call.
    pub max_batch: usize,
    /// Max results from Fs_FindFile.
    pub max_find_results: usize,
    /// Max results from Fs_GrepFile.
    pub max_grep_results: usize,
    /// Max directory listing depth.
    pub max_depth: usize,
    /// Overall per-command timeout.
    pub command_timeout_secs: u64,
    /// Concurrent web requests.
    pub web_concurrency: usize,
    /// Per-request web timeout.
    pub web_timeout_ms: u64,
    /// Max bytes fetched from a single URL.
    pub fetch_max_bytes: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_read_bytes: 8 * 1024 * 1024,
            max_write_bytes: 8 * 1024 * 1024,
            max_batch: 256,
            max_find_results: 500,
            max_grep_results: 500,
            max_depth: 64,
            command_timeout_secs: 120,
            web_concurrency: 8,
            web_timeout_ms: 30_000,
            fetch_max_bytes: 2 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UrlPolicy {
    /// When non-empty, only these domains (suffix match) are allowed.
    pub allowed_domains: Vec<String>,
    /// These domains are always denied (takes precedence over allow list).
    pub blocked_domains: Vec<String>,
    /// Allow private/loopback/link-local IPs. Default false (SSRF protection).
    pub allow_private_ips: bool,
    pub max_redirects: usize,
    pub allowed_schemes: Vec<String>,
}

impl Default for UrlPolicy {
    fn default() -> Self {
        Self {
            allowed_domains: Vec::new(),
            blocked_domains: Vec::new(),
            allow_private_ips: false,
            max_redirects: 5,
            allowed_schemes: vec!["https".into(), "http".into()],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LlmConfig {
    /// OpenAI-compatible base URL, e.g. https://api.openai.com/v1
    pub base_url: Option<String>,
    pub model: Option<String>,
    /// Environment variable holding the API key.
    pub api_key_env: String,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            base_url: None,
            model: None,
            api_key_env: "ACT_LLM_API_KEY".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CompressionConfig {
    /// When true, oversized results are written to the overflow directory and
    /// replaced by a summary envelope.
    pub enabled: bool,
    pub algorithm: SummaryAlgorithm,
    pub max_summary_chars: usize,
    pub llm: LlmConfig,
}

impl Default for CompressionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            algorithm: SummaryAlgorithm::Extractive,
            max_summary_chars: 1000,
            llm: LlmConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputConfig {
    /// Results at or below this size are returned inline.
    pub max_inline_bytes: usize,
    /// Overflow directory relative to the primary root.
    pub overflow_dir: String,
    /// Overflow file retention in days.
    pub overflow_retention_days: u32,
    pub compression: CompressionConfig,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            max_inline_bytes: 32 * 1024,
            overflow_dir: ".act/overflow".into(),
            overflow_retention_days: 7,
            compression: CompressionConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct EnginesConfig {
    /// Enabled search engines by name; default ["duckduckgo", "bing"].
    pub enabled: Vec<String>,
    /// Optional SERPAPI key environment variable value is read at runtime.
    pub google_serpapi_key_env: String,
    pub brave_api_key_env: String,
    pub searxng_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct FsConfig {
    pub delete_mode: DeleteMode,
    pub trash_retention_days: u32,
    /// Fallback encoding when a file is not valid UTF-8 (e.g. "gbk", "shift_jis").
    pub fallback_encoding: String,
    /// Normalize line endings on write: "auto" | "lf" | "crlf" | "none".
    pub eol: String,
}

impl Default for FsConfig {
    fn default() -> Self {
        Self {
            delete_mode: DeleteMode::Trash,
            trash_retention_days: 7,
            fallback_encoding: "gbk".into(),
            eol: "auto".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ActConfig {
    /// Sandboxed roots. Relative entries resolve against the working
    /// directory. Default: the working directory itself.
    pub roots: Vec<PathBuf>,
    /// Protected glob patterns (relative to their matching root).
    pub protected: Vec<String>,
    /// Allow reading protected paths (write/delete is always denied).
    pub allow_read_protected: bool,
    pub fs: FsConfig,
    pub limits: Limits,
    pub url: UrlPolicy,
    pub output: OutputConfig,
    pub engines: EnginesConfig,
    pub capabilities: Capabilities,
}

impl Default for ActConfig {
    fn default() -> Self {
        Self {
            roots: Vec::new(),
            protected: DEFAULT_PROTECTED_GLOBS
                .iter()
                .map(|s| s.to_string())
                .collect(),
            allow_read_protected: false,
            fs: FsConfig::default(),
            limits: Limits::default(),
            url: UrlPolicy::default(),
            output: OutputConfig::default(),
            engines: EnginesConfig {
                enabled: vec!["duckduckgo".into(), "bing".into()],
                ..Default::default()
            },
            capabilities: Capabilities::default(),
        }
    }
}

impl ActConfig {
    /// Load config: defaults <- file (cwd `act.config.json` or `ACT_CONFIG`)
    /// <- environment overrides, then canonicalize roots.
    pub fn load(working_dir: &Path) -> ActResult<Self> {
        let mut cfg = Self::default();

        let file_path = std::env::var_os("ACT_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|| working_dir.join("act.config.json"));
        if file_path.is_file() {
            let raw = std::fs::read_to_string(&file_path).map_err(|e| {
                ActError::Config(format!("failed to read {}: {}", file_path.display(), e))
            })?;
            let parsed: ActConfig = serde_json::from_str(&raw).map_err(|e| {
                ActError::Config(format!("failed to parse {}: {}", file_path.display(), e))
            })?;
            cfg = parsed;
        }

        // Environment overrides.
        if let Some(extra) = std::env::var_os("ACT_EXTRA_ROOTS") {
            for part in split_path_list(&extra.to_string_lossy()) {
                if !part.is_empty() {
                    cfg.roots.push(PathBuf::from(part));
                }
            }
        }
        if let Ok(v) = std::env::var("ACT_DISABLE_NET") {
            if v == "1" || v.eq_ignore_ascii_case("true") {
                cfg.capabilities.cli.net = false;
                cfg.capabilities.mcp.net = false;
            }
        }
        if let Ok(v) = std::env::var("ACT_DISABLE_WRITE") {
            if v == "1" || v.eq_ignore_ascii_case("true") {
                cfg.capabilities.cli.write = false;
                cfg.capabilities.mcp.write = false;
            }
        }

        if cfg.roots.is_empty() {
            cfg.roots.push(working_dir.to_path_buf());
        }

        // Resolve roots to canonical absolute directories.
        let mut resolved = Vec::new();
        for root in &cfg.roots {
            let abs = if root.is_absolute() {
                root.clone()
            } else {
                working_dir.join(root)
            };
            let canon = abs.canonicalize().map_err(|e| {
                ActError::Config(format!(
                    "sandbox root '{}' unavailable: {}",
                    abs.display(),
                    e
                ))
            })?;
            if !canon.is_dir() {
                return Err(ActError::Config(format!(
                    "sandbox root is not a directory: {}",
                    canon.display()
                )));
            }
            resolved.push(crate::guard::path_guard::strip_verbatim(&canon));
        }
        cfg.roots = resolved;
        Ok(cfg)
    }
}

fn split_path_list(list: &str) -> Vec<String> {
    if cfg!(windows) {
        list.split(';').map(|s| s.to_string()).collect()
    } else {
        list.split(':').map(|s| s.to_string()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sane() {
        let cfg = ActConfig::default();
        assert_eq!(cfg.limits.max_batch, 256);
        assert_eq!(cfg.output.max_inline_bytes, 32 * 1024);
        assert_eq!(cfg.fs.delete_mode, DeleteMode::Trash);
        assert!(!cfg.url.allow_private_ips);
        assert!(cfg.protected.iter().any(|p| p == "**/.git/**"));
    }

    #[test]
    fn config_parses_partial_json() {
        let cfg: ActConfig =
            serde_json::from_str(r#"{"limits": {"max_batch": 10}}"#).expect("parse");
        assert_eq!(cfg.limits.max_batch, 10);
        assert_eq!(cfg.limits.max_read_bytes, Limits::default().max_read_bytes);
    }
}
