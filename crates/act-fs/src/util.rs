//! Shared helpers for filesystem commands.

use std::path::Path;

use act_kernel::error::{ActError, ActResult};
use act_kernel::SandboxContext;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// Parse typed parameters with a stable error shape.
pub fn parse_params<T: DeserializeOwned>(params: Value, command: &str) -> ActResult<T> {
    serde_json::from_value(params).map_err(|e| ActError::invalid_params(command, e.to_string()))
}

/// Standard per-item success payload.
pub fn ok_item(path: &str, extra: Value) -> Value {
    let mut item = serde_json::json!({ "path": path, "ok": true });
    if let (Some(base), Some(ext)) = (item.as_object_mut(), extra.as_object()) {
        for (k, v) in ext {
            base.insert(k.clone(), v.clone());
        }
    }
    item
}

/// Standard per-item failure payload.
pub fn err_item(path: &str, err: &ActError) -> Value {
    serde_json::json!({
        "path": path,
        "ok": false,
        "error": err.to_string(),
        "code": err.code(),
    })
}

/// Wrap per-item results into the common envelope.
pub fn envelope(command: &str, results: Vec<Value>) -> Value {
    let succeeded = results
        .iter()
        .filter(|r| r.get("ok").and_then(|o| o.as_bool()).unwrap_or(false))
        .count();
    let failed = results.len() - succeeded;
    serde_json::json!({
        "ok": failed == 0,
        "command": command,
        "results": results,
        "summary": { "succeeded": succeeded, "failed": failed },
    })
}

/// Atomic write: temp file in the same directory, then rename.
pub fn atomic_write(path: &Path, bytes: &[u8]) -> ActResult<()> {
    let dir = path
        .parent()
        .ok_or_else(|| ActError::Other(format!("no parent directory for {}", path.display())))?;
    std::fs::create_dir_all(dir).map_err(ActError::Io)?;
    let nonce = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
    let tmp = dir.join(format!(
        ".{}.{}.acttmp",
        path.file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default(),
        nonce
    ));
    std::fs::write(&tmp, bytes).map_err(ActError::Io)?;
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        // Windows: renaming over an existing file fails; remove then retry.
        if path.exists() {
            std::fs::remove_file(path).map_err(ActError::Io)?;
            std::fs::rename(&tmp, path).map_err(|e2| {
                let _ = std::fs::remove_file(&tmp);
                ActError::Io(e2)
            })?;
            return Ok(());
        }
        return Err(ActError::Io(e));
    }
    Ok(())
}

/// Path as user-facing string: user input base + relative remainder.
pub fn join_user_path(base_input: &str, rel: &Path) -> String {
    let rel_str = rel.to_string_lossy().replace('\\', "/");
    let base = base_input.trim_end_matches('/');
    if rel_str.is_empty() {
        return base.to_string();
    }
    format!("{}/{}", base, rel_str)
}

/// Ensure the target is not a sandbox root and does not contain a sandbox
/// root (e.g. moving/renaming a parent directory of a root).
pub fn ensure_not_root(ctx: &SandboxContext, abs: &Path) -> ActResult<()> {
    if ctx.is_root(abs) {
        return Err(ActError::permission_denied(
            "SandboxRoot",
            format!("refusing to modify sandbox root '{}'", abs.display()),
        ));
    }
    for root in ctx.roots() {
        // A root strictly inside `abs` means `abs` contains the root.
        if act_kernel::guard::path_guard::strip_prefix_ci(root, abs).is_some() {
            return Err(ActError::permission_denied(
                "SandboxRoot",
                format!("'{}' contains a sandbox root", abs.display()),
            ));
        }
    }
    Ok(())
}
