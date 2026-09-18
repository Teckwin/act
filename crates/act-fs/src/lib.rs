//! act-fs: filesystem command extensions for Agent Core Tools.
//!
//! Registers the 15 `Fs_*` built-ins into the kernel's CommandManager. Every
//! handler resolves paths through `SandboxContext` only; there is no direct
//! filesystem access outside the sandbox roots.

pub mod commands;
pub mod encoding;
pub mod trash;
pub mod util;

pub mod prelude {
    pub use crate::encoding::{decode, encode, normalize_eol};
}

use act_kernel::error::ActResult;
use act_kernel::{CommandDef, CommandManager};

/// Register all Fs_* commands into the manager.
pub fn register_all(manager: &CommandManager) -> ActResult<()> {
    for def in definitions() {
        manager.register(def)?;
    }
    Ok(())
}

/// All Fs_* command definitions (used by registration and docs).
pub fn definitions() -> Vec<CommandDef> {
    vec![
        commands::read::definition(),
        commands::write::write_definition(),
        commands::write::append_definition(),
        commands::write::create_definition(),
        commands::edit::definition(),
        commands::remove::remove_file_definition(),
        commands::remove::remove_dir_definition(),
        commands::moves::move_file_definition(),
        commands::moves::move_dir_definition(),
        commands::moves::copy_file_definition(),
        commands::moves::copy_dir_definition(),
        commands::dir::create_dir_definition(),
        commands::dir::list_dir_definition(),
        commands::dir::file_info_definition(),
        commands::find::definition(),
        commands::grep::definition(),
    ]
    .into_iter()
    .collect::<Result<Vec<_>, _>>()
    .expect("builtin Fs definitions must be valid")
}
