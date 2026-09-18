//! PathGuard: filesystem sandbox with deny-by-default containment checks.
//!
//! Resolution strategy for every path crossing the kernel boundary:
//! 1. lexical normalization (resolve `.` and `..` without touching the fs)
//! 2. the lexically-normalized path must be inside one of the sandbox roots
//!    (this stops `../` traversal before any filesystem access)
//! 3. incremental canonicalization: every existing component is canonicalized,
//!    and symlink targets are re-validated against the roots (stops symlink
//!    escape); non-existent trailing components may only be `Normal`
//! 4. Windows verbatim prefixes (`\\?\`, `\\?\UNC\`) are stripped and path
//!    comparison is case-insensitive on Windows

use std::path::{Component, Path, PathBuf};

use crate::error::{ActError, ActResult};

pub const MAX_PATH_DEPTH: usize = 100;

#[derive(Debug, Clone)]
pub struct PathGuard {
    /// Canonical sandbox roots (verbatim prefixes stripped).
    roots: Vec<PathBuf>,
}

/// Strip Windows verbatim prefixes so containment checks compare plain paths.
pub fn strip_verbatim(path: &Path) -> PathBuf {
    let s = path.as_os_str().to_string_lossy();
    let s = s.as_ref();
    if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{}", rest))
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        path.to_path_buf()
    }
}

/// Path equality honouring case-insensitivity on Windows.
fn component_eq(a: &Component<'_>, b: &Component<'_>) -> bool {
    if cfg!(windows) {
        a.as_os_str().to_string_lossy().to_lowercase()
            == b.as_os_str().to_string_lossy().to_lowercase()
    } else {
        a.as_os_str() == b.as_os_str()
    }
}

/// Case-insensitive-on-Windows `strip_prefix`. Returns the remainder.
pub fn strip_prefix_ci(path: &Path, base: &Path) -> Option<PathBuf> {
    let mut pi = path.components();
    let mut bi = base.components();
    loop {
        match bi.next() {
            None => return Some(pi.collect()),
            Some(b) => match pi.next() {
                None => return None,
                Some(a) => {
                    if !component_eq(&a, &b) {
                        return None;
                    }
                }
            },
        }
    }
}

/// Relative path of `path` under `root` (forward-slashed), or empty when equal.
pub fn rel_to_root(path: &Path, root: &Path) -> PathBuf {
    strip_prefix_ci(path, root).unwrap_or_else(|| PathBuf::new())
}

/// Lexically normalize a path: remove `.` segments, apply `..` (without
/// touching the filesystem), drop redundant separators.
pub fn lexical_normalize(path: &Path) -> PathBuf {
    let mut prefix: Option<PathBuf> = None;
    let mut normal: Vec<std::ffi::OsString> = Vec::new();
    for component in path.components() {
        match component {
            Component::Prefix(p) => {
                prefix = Some(PathBuf::from(p.as_os_str()));
                normal.clear();
            }
            Component::RootDir => {
                normal.clear();
            }
            Component::CurDir => {}
            Component::ParentDir => {
                // Popping past the top of an absolute path stays at its root.
                normal.pop();
            }
            Component::Normal(seg) => normal.push(seg.to_os_string()),
        }
    }
    let mut out = match prefix {
        Some(p) => {
            let mut o = p;
            o.push(std::path::MAIN_SEPARATOR.to_string());
            o
        }
        None if path.has_root() => PathBuf::from(std::path::MAIN_SEPARATOR.to_string()),
        None => PathBuf::new(),
    };
    for seg in normal {
        out.push(seg);
    }
    out
}

impl PathGuard {
    /// Build a guard from already-canonical roots.
    pub fn new(roots: Vec<PathBuf>) -> ActResult<Self> {
        if roots.is_empty() {
            return Err(ActError::Config(
                "at least one sandbox root is required".into(),
            ));
        }
        let mut stripped = Vec::with_capacity(roots.len());
        for root in roots {
            let canon = root.canonicalize().map_err(|e| {
                ActError::Config(format!("root '{}' unavailable: {}", root.display(), e))
            })?;
            if !canon.is_dir() {
                return Err(ActError::Config(format!(
                    "root is not a directory: {}",
                    canon.display()
                )));
            }
            stripped.push(strip_verbatim(&canon));
        }
        Ok(Self { roots: stripped })
    }

    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    pub fn primary_root(&self) -> &Path {
        &self.roots[0]
    }

    /// Resolve a user-supplied path against the sandbox roots.
    /// Relative paths resolve against the primary root.
    /// Returns (absolute canonical path, matched root index).
    pub fn resolve(&self, input: &str) -> ActResult<(PathBuf, usize)> {
        let trimmed = input.trim();
        if trimmed.is_empty() {
            return Err(ActError::invalid_params("path", "empty path"));
        }
        if trimmed.contains('\0') {
            return Err(ActError::invalid_params("path", "path contains NUL byte"));
        }

        let raw = Path::new(trimmed);
        let candidate = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            self.primary_root().join(raw)
        };
        let normalized = strip_verbatim(&lexical_normalize(&candidate));

        if normalized.components().count() > MAX_PATH_DEPTH {
            return Err(ActError::LimitExceeded {
                reason: format!("path depth exceeds {}", MAX_PATH_DEPTH),
            });
        }

        // Lexical containment gate (before any fs access).
        let (root_idx, lexical_rel) = self
            .match_root(&normalized)
            .ok_or_else(|| deny(&normalized, &self.roots))?;

        // Incremental canonicalization with symlink re-validation.
        let root = self.roots[root_idx].clone();
        let mut current = root.clone();
        let remainder: Vec<std::ffi::OsString> = lexical_rel
            .components()
            .filter_map(|c| match c {
                Component::Normal(seg) => Some(seg.to_os_string()),
                _ => None,
            })
            .collect();

        let mut matched_root = root_idx;
        for (i, seg) in remainder.iter().enumerate() {
            current.push(seg);
            match std::fs::symlink_metadata(&current) {
                Ok(meta) => {
                    if meta.file_type().is_symlink() {
                        let canon = std::fs::canonicalize(&current).map_err(|e| {
                            ActError::Io(std::io::Error::new(
                                e.kind(),
                                format!("failed to canonicalize '{}': {}", current.display(), e),
                            ))
                        })?;
                        let canon = strip_verbatim(&canon);
                        match self.match_root(&canon) {
                            Some((idx, rel)) => {
                                matched_root = idx;
                                // Restart from the canonical location.
                                current = self.roots[idx].join(rel);
                            }
                            None => {
                                return Err(ActError::permission_denied(
                                    "PathGuard",
                                    format!(
                                        "symlink '{}' resolves outside sandbox roots: {}",
                                        current.display(),
                                        canon.display()
                                    ),
                                ));
                            }
                        }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // Remaining components do not exist; they must all be
                    // Normal segments (guaranteed by lexical normalization),
                    // so simply append and finish.
                    for later in remainder.iter().skip(i + 1) {
                        current.push(later);
                    }
                    return Ok((current, matched_root));
                }
                Err(e) => {
                    return Err(ActError::Execution {
                        command: "resolve_path".into(),
                        detail: format!("cannot inspect '{}': {}", current.display(), e),
                    });
                }
            }
        }
        Ok((current, matched_root))
    }

    /// True when `path` equals one of the sandbox roots themselves.
    pub fn is_root(&self, path: &Path) -> bool {
        self.roots.iter().any(|r| crate::guard::path_eq(path, r))
    }

    /// True when `path` (already canonical) is inside any root.
    pub fn is_within(&self, path: &Path) -> bool {
        self.match_root(path).is_some()
    }

    fn match_root(&self, path: &Path) -> Option<(usize, PathBuf)> {
        for (idx, root) in self.roots.iter().enumerate() {
            if let Some(rel) = strip_prefix_ci(path, root) {
                return Some((idx, rel));
            }
        }
        None
    }
}

fn deny(path: &Path, roots: &[PathBuf]) -> ActError {
    ActError::permission_denied(
        "PathGuard",
        format!(
            "path '{}' is outside sandbox roots [{}]",
            path.display(),
            roots
                .iter()
                .map(|r| r.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guard_in(dir: &Path) -> PathGuard {
        PathGuard::new(vec![dir.to_path_buf()]).expect("guard")
    }

    #[test]
    fn relative_inside_ok() {
        let tmp = tempfile::tempdir().unwrap();
        let g = guard_in(tmp.path());
        let (p, idx) = g.resolve("a/b.txt").unwrap();
        assert_eq!(idx, 0);
        assert!(p.starts_with(tmp.path()));
        assert_eq!(p, tmp.path().join("a/b.txt"));
    }

    #[test]
    fn dotdot_traversal_denied() {
        let tmp = tempfile::tempdir().unwrap();
        let g = guard_in(tmp.path());
        let err = g.resolve("../../etc/passwd").unwrap_err();
        assert!(matches!(
            err,
            ActError::PermissionDenied {
                guard: "PathGuard",
                ..
            }
        ));
    }

    #[test]
    fn absolute_outside_denied() {
        let tmp = tempfile::tempdir().unwrap();
        let g = guard_in(tmp.path());
        let outside = if cfg!(windows) {
            r"C:\Windows\System32"
        } else {
            "/etc"
        };
        assert!(g.resolve(outside).is_err());
    }

    #[test]
    fn absolute_inside_ok() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("f.txt"), b"x").unwrap();
        let g = guard_in(tmp.path());
        let abs = tmp.path().join("f.txt");
        let (p, _) = g.resolve(abs.to_str().unwrap()).unwrap();
        let expected = strip_verbatim(&std::fs::canonicalize(&abs).unwrap());
        assert_eq!(p, expected);
    }

    #[test]
    fn inner_dotdot_lands_inside_ok() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join("a/b")).unwrap();
        let g = guard_in(tmp.path());
        let (p, _) = g.resolve("a/b/../c.txt").unwrap();
        assert_eq!(p, tmp.path().join("a").join("c.txt"));
    }

    #[test]
    fn nonexistent_nested_ok_but_rooted() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("src")).unwrap();
        let g = guard_in(tmp.path());
        let (p, _) = g.resolve("src/new/deep/file.rs").unwrap();
        assert!(p.starts_with(tmp.path()));
        assert_eq!(p, tmp.path().join("src/new/deep/file.rs"));
    }

    #[test]
    #[cfg(unix)]
    fn symlink_escape_denied() {
        let tmp = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("secret.txt"), b"s").unwrap();
        std::os::unix::fs::symlink(outside.path(), tmp.path().join("link")).unwrap();
        let g = guard_in(tmp.path());
        let err = g.resolve("link/secret.txt").unwrap_err();
        assert!(
            matches!(err, ActError::PermissionDenied { .. }),
            "got: {err:?}"
        );
    }

    #[test]
    #[cfg(unix)]
    fn symlink_inside_ok() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir(tmp.path().join("real")).unwrap();
        std::fs::write(tmp.path().join("real/f.txt"), b"s").unwrap();
        std::os::unix::fs::symlink("real", tmp.path().join("alias")).unwrap();
        let g = guard_in(tmp.path());
        let (p, _) = g.resolve("alias/f.txt").unwrap();
        assert!(p.ends_with("real/f.txt"));
    }

    #[test]
    fn verbatim_prefix_accepted_on_windows() {
        if !cfg!(windows) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("f.txt"), b"x").unwrap();
        let g = guard_in(tmp.path());
        let canon = std::fs::canonicalize(tmp.path().join("f.txt")).unwrap();
        let verbatim = canon.to_string_lossy().to_string();
        assert!(verbatim.starts_with(r"\\?\"));
        let (p, _) = g.resolve(&verbatim).unwrap();
        assert!(g.is_within(&p));
    }

    #[test]
    fn case_insensitive_on_windows() {
        if !cfg!(windows) {
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let canon = std::fs::canonicalize(tmp.path()).unwrap();
        let g = guard_in(tmp.path());
        let upper = canon.to_string_lossy().to_uppercase();
        assert!(
            g.resolve(&upper).is_ok(),
            "case-variant root must be accepted on Windows"
        );
    }

    #[test]
    fn root_itself_detected() {
        let tmp = tempfile::tempdir().unwrap();
        let g = guard_in(tmp.path());
        let (p, _) = g.resolve(".").unwrap();
        assert!(g.is_root(&p));
    }

    #[test]
    fn empty_and_nul_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let g = guard_in(tmp.path());
        assert!(matches!(
            g.resolve("  "),
            Err(ActError::InvalidParams { .. })
        ));
        assert!(matches!(
            g.resolve("a\0b"),
            Err(ActError::InvalidParams { .. })
        ));
    }

    #[test]
    fn second_root_matches() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        std::fs::write(b.path().join("x.txt"), b"1").unwrap();
        let g = PathGuard::new(vec![a.path().to_path_buf(), b.path().to_path_buf()]).unwrap();
        let abs = std::fs::canonicalize(b.path().join("x.txt")).unwrap();
        let (p, idx) = g.resolve(abs.to_str().unwrap()).unwrap();
        assert_eq!(idx, 1);
        assert!(g.is_within(&p));
        // Relative still resolves against primary root.
        let (_, idx2) = g.resolve("y.txt").unwrap();
        assert_eq!(idx2, 0);
    }

    #[test]
    fn lexical_normalize_cases() {
        assert_eq!(
            lexical_normalize(Path::new("/a/b/../c")),
            PathBuf::from("/a/c")
        );
        assert_eq!(
            lexical_normalize(Path::new("/a/./b")),
            PathBuf::from("/a/b")
        );
        assert_eq!(lexical_normalize(Path::new("/../..")), PathBuf::from("/"));
        if cfg!(windows) {
            assert_eq!(
                lexical_normalize(Path::new(r"C:\a\..\b")),
                PathBuf::from(r"C:\b")
            );
        }
    }
}
