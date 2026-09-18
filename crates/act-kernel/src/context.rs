//! Invocation context and the sandbox context handed to command handlers.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use url::Url;

use crate::config::ActConfig;
use crate::error::ActResult;
use crate::guard::path_eq;
use crate::guard::path_guard::PathGuard;
use crate::guard::protect_guard::{ProtectGuard, ProtectOp};
use crate::guard::url_guard::UrlGuard;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InvokeMode {
    Cli,
    Mcp,
}

impl InvokeMode {
    pub fn label(&self) -> &'static str {
        match self {
            InvokeMode::Cli => "cli",
            InvokeMode::Mcp => "mcp",
        }
    }
}

/// Per-invocation metadata.
#[derive(Debug, Clone)]
pub struct InvokeContext {
    pub mode: InvokeMode,
    pub request_id: String,
    pub started_at: DateTime<Utc>,
}

impl InvokeContext {
    pub fn new(mode: InvokeMode) -> Self {
        Self {
            mode,
            request_id: uuid::Uuid::new_v4().to_string(),
            started_at: Utc::now(),
        }
    }
}

/// The ONLY capability object handlers receive. Path and URL validation are
/// funnelled through here so extensions cannot bypass the kernel guards.
pub struct SandboxContext {
    pub config: Arc<ActConfig>,
    path_guard: Arc<PathGuard>,
    url_guard: UrlGuard,
    protect_guard: ProtectGuard,
}

impl SandboxContext {
    pub fn new(
        config: Arc<ActConfig>,
        path_guard: Arc<PathGuard>,
        url_guard: UrlGuard,
        protect_guard: ProtectGuard,
    ) -> Self {
        Self {
            config,
            path_guard,
            url_guard,
            protect_guard,
        }
    }

    pub fn roots(&self) -> &[PathBuf] {
        self.path_guard.roots()
    }

    pub fn primary_root(&self) -> &Path {
        self.path_guard.primary_root()
    }

    /// Validate and resolve a path; returns (absolute, root index).
    pub fn resolve_path(&self, input: &str) -> ActResult<(PathBuf, usize)> {
        self.path_guard.resolve(input)
    }

    /// Validate a URL (scheme/domain/SSRF). Callers must re-validate on every
    /// redirect hop.
    pub async fn check_url(&self, raw: &str) -> ActResult<Url> {
        self.url_guard.check(raw).await
    }

    pub fn url_policy(&self) -> &crate::config::UrlPolicy {
        self.url_guard.policy()
    }

    /// True when the path equals a sandbox root itself.
    pub fn is_root(&self, path: &Path) -> bool {
        self.roots().iter().any(|r| path_eq(path, r))
    }

    /// Root-relative path for the given absolute path, if inside a root.
    pub fn rel_to_root(&self, path: &Path) -> Option<PathBuf> {
        for root in self.roots() {
            if let Some(rel) = crate::guard::path_guard::strip_prefix_ci(path, root) {
                return Some(rel);
            }
        }
        None
    }

    /// Protected-path check (also enforced pre-execution by the verifier).
    pub fn protect_check(&self, abs: &Path, op: ProtectOp) -> ActResult<()> {
        let rel = self.rel_to_root(abs).unwrap_or_default();
        self.protect_guard.check_rel(&rel, op)
    }

    /// Overflow directory (absolute) for oversized results.
    pub fn overflow_dir(&self) -> PathBuf {
        self.primary_root().join(&self.config.output.overflow_dir)
    }
}
