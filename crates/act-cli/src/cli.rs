//! CLI plumbing: manager construction and the single dynamic command path.
//! There are no builtin subcommands — every verb (including Sys_List /
//! Sys_Verify / Sys_Schema / Sys_Package / Sys_Install / Sys_Serve) is a
//! registered command reached via `act <Command|alias> --flags`.

use act_kernel::error::{ActError, ActResult};
use act_kernel::{ActConfig, CommandManager};
use std::path::PathBuf;

/// Build the manager from explicit config/roots (config file defaults to the
/// `ACT_CONFIG` env var or `<cwd>/act.config.json`).
pub fn build_manager_with(
    config: Option<PathBuf>,
    roots: Vec<PathBuf>,
) -> ActResult<CommandManager> {
    if let Some(path) = &config {
        let abs = if path.is_absolute() {
            path.clone()
        } else {
            std::env::current_dir().unwrap_or_default().join(path)
        };
        std::env::set_var("ACT_CONFIG", &abs);
    }
    let cwd = std::env::current_dir().map_err(ActError::Io)?;
    let mut config = ActConfig::load(&cwd)?;
    for root in &roots {
        let abs = if root.is_absolute() {
            root.clone()
        } else {
            cwd.join(root)
        };
        let canon = abs.canonicalize().map_err(|e| {
            ActError::Config(format!("--root '{}' unavailable: {}", abs.display(), e))
        })?;
        if !config.roots.contains(&canon) {
            config.roots.push(canon);
        }
    }
    let manager = CommandManager::new(config)?;
    act_fs::register_all(&manager)?;
    act_web::register_all(&manager)?;
    crate::sys::register_all(&manager)?;
    Ok(manager)
}
pub fn print_json(value: &serde_json::Value) {
    match serde_json::to_string_pretty(value) {
        Ok(text) => println!("{text}"),
        Err(e) => eprintln!("serialize output failed: {e}"),
    }
}
