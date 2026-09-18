//! Permission verifier pipeline: five guards, deny-by-default.

pub mod capability_guard;
pub mod limit_guard;
pub mod path_guard;
pub mod protect_guard;
pub mod schema_guard;
pub mod url_guard;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::Value;
use url::Url;

use crate::config::{ActConfig, Limits};
use crate::error::{ActError, ActResult};
use crate::registry::{Capability, CommandDef};

pub use protect_guard::ProtectOp;

/// Result of a successful verification pass.
#[derive(Debug, Clone)]
pub struct Verification {
    /// (raw input, resolved absolute path, root index)
    pub paths: Vec<(String, PathBuf, usize)>,
    /// (raw input, checked URL)
    pub urls: Vec<(String, Url)>,
}

/// The five-guard permission verifier. Runs before any handler.
pub struct PermissionVerifier {
    path_guard: Arc<path_guard::PathGuard>,
    url_guard: url_guard::UrlGuard,
    protect_guard: protect_guard::ProtectGuard,
    limits: Limits,
    capabilities: crate::config::Capabilities,
}

impl PermissionVerifier {
    pub fn new(config: &ActConfig) -> ActResult<Self> {
        let path_guard = Arc::new(path_guard::PathGuard::new(config.roots.clone())?);
        let url_guard = url_guard::UrlGuard::new(config.url.clone());
        let protect_guard = protect_guard::ProtectGuard::from_config(
            &config.protected,
            config.allow_read_protected,
        );
        Ok(Self {
            path_guard,
            url_guard,
            protect_guard,
            limits: config.limits.clone(),
            capabilities: config.capabilities.clone(),
        })
    }

    pub fn path_guard(&self) -> &Arc<path_guard::PathGuard> {
        &self.path_guard
    }

    pub async fn verify(
        &self,
        def: &CommandDef,
        params: &Value,
        mode: &crate::context::InvokeMode,
    ) -> ActResult<Verification> {
        // Guard 1: capability per invocation mode.
        capability_guard::check(&def.capability, &self.capabilities, mode)?;

        // Guard 2: schema conformance (required/type/enum/range) — enforced
        // uniformly for every channel: CLI parser, MCP JSON, direct exec.
        schema_guard::validate(def, params)?;

        // Guard 3 + 4: path containment and protected globs.
        let mut paths = Vec::new();
        for field in &def.path_fields {
            for raw in crate::param::extract_strings(params, field) {
                let (abs, root_idx) = self.path_guard.resolve(&raw)?;
                let root = &self.path_guard.roots()[root_idx];
                let rel = path_guard::rel_to_root(&abs, root);
                let op = match def.capability {
                    Capability::Write => ProtectOp::Write,
                    _ => ProtectOp::Read,
                };
                self.protect_guard.check_rel(&rel, op)?;
                paths.push((raw, abs, root_idx));
            }
        }

        // Guard 5: URL policy (scheme/domain/private-IP/SSRF).
        let mut urls = Vec::new();
        for field in &def.url_fields {
            for raw in crate::param::extract_strings(params, field) {
                let checked = self.url_guard.check(&raw).await?;
                urls.push((raw, checked));
            }
        }

        // Guard 6: numeric limits and batch size.
        let total_items = paths.len() + urls.len();
        if total_items > self.limits.max_batch {
            return Err(ActError::LimitExceeded {
                reason: format!(
                    "{} items exceed max_batch={} (paths={}, urls={})",
                    total_items,
                    self.limits.max_batch,
                    paths.len(),
                    urls.len()
                ),
            });
        }
        limit_guard::check_numeric(params, &self.limits)?;

        Ok(Verification { paths, urls })
    }
}

/// Path helpers shared with the sandbox context.
pub fn is_root_path(path: &Path, roots: &[PathBuf]) -> bool {
    roots.iter().any(|root| path_eq(path, root))
}

/// Case-insensitive path equality on Windows, exact elsewhere.
pub fn path_eq(a: &Path, b: &Path) -> bool {
    if cfg!(windows) {
        a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
    } else {
        a == b
    }
}
