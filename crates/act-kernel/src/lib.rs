//! act-kernel: the command kernel for Agent Core Tools.
//!
//! Architecture: CommandManager (registry + permission verifier + executor +
//! output policy + audit). Command extensions (`act-fs`, `act-web`) register
//! built-in commands and can never be invoked outside the manager, which
//! enforces the five security guards on every call.

pub mod audit;
pub mod builder;
pub mod config;
pub mod context;
pub mod error;
pub mod executor;
pub mod guard;
pub mod manager;
pub mod name;
pub mod output;
pub mod param;
pub mod registry;
pub mod summarize;

pub use builder::{CommandBuilder, Param, ParamType};
pub use config::ActConfig;
pub use context::{InvokeMode, SandboxContext};
pub use error::{ActError, ActResult};
pub use manager::CommandManager;
pub use registry::{Capability, CommandDef, CommandHandler, CommandInfo, OutputSpec, Verify};
