//! ProtectGuard: glob-based protection for sensitive paths.
//!
//! Applies to paths relative to their sandbox root. Write/delete is always
//! denied for protected paths; reads are denied unless explicitly allowed.

use std::path::Path;

use globset::{Glob, GlobSetBuilder};

use crate::error::{ActError, ActResult};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtectOp {
    Read,
    Write,
}

pub struct ProtectGuard {
    set: globset::GlobSet,
    allow_read: bool,
}

impl ProtectGuard {
    pub fn from_config(patterns: &[String], allow_read: bool) -> Self {
        let mut builder = GlobSetBuilder::new();
        for pattern in patterns {
            match Glob::new(pattern) {
                Ok(glob) => {
                    builder.add(glob);
                }
                Err(e) => {
                    tracing::warn!("invalid protected glob '{}': {}", pattern, e);
                }
            }
        }
        Self {
            set: builder.build().unwrap_or_default(),
            allow_read,
        }
    }

    /// Check a root-relative path (any separator style).
    pub fn check_rel(&self, rel: &Path, op: ProtectOp) -> ActResult<()> {
        if op == ProtectOp::Read && self.allow_read {
            return Ok(());
        }
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        if rel_str.is_empty() {
            // The root itself: protected only for writes on the root path;
            // handled by is_root checks in the fs commands.
            return Ok(());
        }
        if self.set.is_match(&rel_str) {
            return Err(ActError::permission_denied(
                "ProtectGuard",
                format!("path '{}' is protected ({op:?})", rel_str),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guard() -> ProtectGuard {
        ProtectGuard::from_config(
            &["**/.git/**".into(), ".env".into(), "**/*.pem".into()],
            false,
        )
    }

    #[test]
    fn git_internals_write_denied() {
        let err = guard()
            .check_rel(Path::new(".git/config"), ProtectOp::Write)
            .unwrap_err();
        assert!(matches!(err, ActError::PermissionDenied { .. }));
    }

    #[test]
    fn git_internals_read_denied_by_default() {
        let err = guard()
            .check_rel(Path::new("a/.git/HEAD"), ProtectOp::Read)
            .unwrap_err();
        assert!(matches!(err, ActError::PermissionDenied { .. }));
    }

    #[test]
    fn read_allowed_when_flagged() {
        let g = ProtectGuard::from_config(&["**/.git/**".into()], true);
        g.check_rel(Path::new(".git/HEAD"), ProtectOp::Read)
            .expect("read allowed");
        assert!(g
            .check_rel(Path::new(".git/HEAD"), ProtectOp::Write)
            .is_err());
    }

    #[test]
    fn normal_paths_pass() {
        guard()
            .check_rel(Path::new("src/main.rs"), ProtectOp::Write)
            .expect("ok");
        guard()
            .check_rel(Path::new("docs/a b.md"), ProtectOp::Read)
            .expect("ok");
    }

    #[test]
    fn env_and_pem_denied() {
        assert!(guard()
            .check_rel(Path::new(".env"), ProtectOp::Read)
            .is_err());
        assert!(guard()
            .check_rel(Path::new("certs/server.pem"), ProtectOp::Read)
            .is_err());
    }
}
