//! Unified error taxonomy for the ACT kernel.

use thiserror::Error;

/// All errors produced by the kernel and command extensions.
#[derive(Debug, Error)]
pub enum ActError {
    #[error("unknown command: {0}")]
    UnknownCommand(String),

    #[error("invalid command name '{name}': expected '<Domain>_<Action>' in PascalCase")]
    InvalidName { name: String },

    #[error("domain '{domain}' is not registered (known domains: {known})")]
    UnknownDomain { domain: String, known: String },

    #[error("command '{existing}' is already registered")]
    DuplicateCommand { existing: String },

    #[error("invalid parameters for {command}: {detail}")]
    InvalidParams { command: String, detail: String },

    #[error("permission denied [{guard}]: {reason}")]
    PermissionDenied { guard: &'static str, reason: String },

    #[error("limit exceeded: {reason}")]
    LimitExceeded { reason: String },

    #[error("capability '{capability}' is disabled for mode '{mode}'")]
    CapabilityDisabled { capability: String, mode: String },

    #[error("execution of {command} failed: {detail}")]
    Execution { command: String, detail: String },

    #[error("output contract violation in {command}: {detail}")]
    OutputContract { command: String, detail: String },

    #[error("command timed out after {0:?}")]
    Timeout(std::time::Duration),

    #[error("config error: {0}")]
    Config(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}

impl ActError {
    /// Stable machine-readable error code used by CLI exit codes and MCP results.
    pub fn code(&self) -> &'static str {
        match self {
            ActError::UnknownCommand(_) => "unknown_command",
            ActError::InvalidName { .. } => "invalid_name",
            ActError::UnknownDomain { .. } => "unknown_domain",
            ActError::DuplicateCommand { .. } => "duplicate_command",
            ActError::InvalidParams { .. } => "invalid_params",
            ActError::PermissionDenied { .. } => "permission_denied",
            ActError::LimitExceeded { .. } => "limit_exceeded",
            ActError::CapabilityDisabled { .. } => "capability_disabled",
            ActError::Execution { .. } => "execution_failed",
            ActError::OutputContract { .. } => "contract_violation",
            ActError::Timeout(_) => "timeout",
            ActError::Config(_) => "config_error",
            ActError::Io(_) => "io_error",
            ActError::Other(_) => "other",
        }
    }

    /// Exit code for the CLI binary.
    pub fn exit_code(&self) -> i32 {
        match self {
            ActError::UnknownCommand(_) => 3,
            ActError::InvalidName { .. }
            | ActError::UnknownDomain { .. }
            | ActError::DuplicateCommand { .. }
            | ActError::InvalidParams { .. } => 4,
            ActError::PermissionDenied { .. }
            | ActError::LimitExceeded { .. }
            | ActError::CapabilityDisabled { .. } => 2,
            ActError::Execution { .. }
            | ActError::OutputContract { .. }
            | ActError::Io(_)
            | ActError::Other(_) => 5,
            ActError::Timeout(_) => 6,
            ActError::Config(_) => 7,
        }
    }

    pub fn permission_denied(guard: &'static str, reason: impl Into<String>) -> Self {
        ActError::PermissionDenied {
            guard,
            reason: reason.into(),
        }
    }

    pub fn invalid_params(command: impl Into<String>, detail: impl Into<String>) -> Self {
        ActError::InvalidParams {
            command: command.into(),
            detail: detail.into(),
        }
    }

    pub fn execution(command: impl Into<String>, detail: impl Into<String>) -> Self {
        ActError::Execution {
            command: command.into(),
            detail: detail.into(),
        }
    }

    pub fn output_contract(command: impl Into<String>, detail: impl Into<String>) -> Self {
        ActError::OutputContract {
            command: command.into(),
            detail: detail.into(),
        }
    }
}

pub type ActResult<T> = Result<T, ActError>;
