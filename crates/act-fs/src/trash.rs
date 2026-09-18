//! Trash: safer deletes. Files/directories are moved under
//! `<primary-root>/.act/trash/<timestamp>-<id>/<root-index>/<relative-path>`
//! instead of being destroyed when `delete_mode = "trash"` (the default).

use std::path::{Path, PathBuf};

use act_kernel::error::{ActError, ActResult};
use act_kernel::SandboxContext;
use chrono::Utc;

pub struct Trash;

impl Trash {
    pub fn enabled(ctx: &SandboxContext) -> bool {
        ctx.config.fs.delete_mode == act_kernel::config::DeleteMode::Trash
    }

    /// Move a file or directory into the trash area. Returns the trash path.
    pub fn stash(ctx: &SandboxContext, abs: &Path, root_idx: usize) -> ActResult<PathBuf> {
        let rel = act_kernel::guard::path_guard::strip_prefix_ci(abs, &ctx.roots()[root_idx])
            .unwrap_or_else(|| PathBuf::from("unnamed"));
        let stamp = Utc::now().format("%Y%m%d-%H%M%S");
        let id = uuid::Uuid::new_v4().simple().to_string()[..8].to_string();
        let target_dir = ctx
            .primary_root()
            .join(".act/trash")
            .join(format!("{}-{}", stamp, id))
            .join(root_idx.to_string())
            .join(&rel);
        if let Some(parent) = target_dir.parent() {
            std::fs::create_dir_all(parent).map_err(ActError::Io)?;
        }
        match std::fs::rename(abs, &target_dir) {
            Ok(()) => {}
            Err(e)
                if e.raw_os_error() == Some(18)
                    || e.kind() == std::io::ErrorKind::CrossesDevices =>
            {
                // Cross-volume fallback: copy then delete.
                copy_recursive(abs, &target_dir)?;
                if abs.is_dir() {
                    std::fs::remove_dir_all(abs).map_err(ActError::Io)?;
                } else {
                    std::fs::remove_file(abs).map_err(ActError::Io)?;
                }
            }
            Err(e) => {
                return Err(ActError::Io(e));
            }
        }
        Self::purge_expired(ctx);
        Ok(target_dir)
    }

    /// Remove trash groups older than the configured retention.
    pub fn purge_expired(ctx: &SandboxContext) {
        let days = ctx.config.fs.trash_retention_days;
        if days == 0 {
            return;
        }
        let cutoff = Utc::now() - chrono::Duration::days(days as i64);
        let base = ctx.primary_root().join(".act/trash");
        let Ok(entries) = std::fs::read_dir(&base) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(ts) = name.split('-').next() {
                if let Ok(stamp) = chrono::NaiveDateTime::parse_from_str(ts, "%Y%m%d%H%M%S") {
                    // stamp is date-only precision group "YYYYmmdd-HHMMSS"
                    let _ = stamp;
                }
            }
            // Fall back to modified time.
            if let Ok(meta) = entry.metadata() {
                if let Ok(modified) = meta.modified() {
                    let modified_utc: chrono::DateTime<Utc> = modified.into();
                    if modified_utc < cutoff {
                        let _ = std::fs::remove_dir_all(entry.path());
                    }
                }
            }
        }
    }
}

/// Copy a file or a whole directory tree.
pub fn copy_recursive(from: &Path, to: &Path) -> ActResult<()> {
    if from.is_dir() {
        walkdir::WalkDir::new(from)
            .into_iter()
            .filter_map(|e| e.ok())
            .for_each(|entry| {
                let rel = entry.path().strip_prefix(from).unwrap_or(entry.path());
                let target = to.join(rel);
                if entry.path().is_dir() {
                    let _ = std::fs::create_dir_all(&target);
                } else if let Some(parent) = target.parent() {
                    let _ = std::fs::create_dir_all(parent);
                    let _ = std::fs::copy(entry.path(), &target);
                }
            });
        Ok(())
    } else {
        if let Some(parent) = to.parent() {
            std::fs::create_dir_all(parent).map_err(ActError::Io)?;
        }
        std::fs::copy(from, to).map_err(ActError::Io)?;
        Ok(())
    }
}
