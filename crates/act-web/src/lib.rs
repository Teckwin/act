//! act-web: web command extensions for Agent Core Tools.
//!
//! Registers `Web_Fetch`, `Web_Search` and `Web_Research`. All HTTP access
//! goes through the kernel UrlGuard (per request and per redirect hop).

pub mod commands;
pub mod engines;
pub mod http;
pub mod markdown;

use act_kernel::error::ActResult;
use act_kernel::{CommandDef, CommandManager};

/// Register all Web_* commands into the manager.
pub fn register_all(manager: &CommandManager) -> ActResult<()> {
    for def in definitions() {
        manager.register(def)?;
    }
    Ok(())
}

pub fn definitions() -> Vec<CommandDef> {
    vec![
        commands::fetch::definition(),
        commands::search::definition(),
        commands::research::definition(),
    ]
    .into_iter()
    .collect::<Result<Vec<_>, _>>()
    .expect("builtin Web definitions must be valid")
}
