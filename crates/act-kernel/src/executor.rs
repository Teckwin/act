//! CommandExecutor: runs a handler with a hard timeout.

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;

use crate::context::SandboxContext;
use crate::error::{ActError, ActResult};
use crate::registry::CommandDef;

pub struct CommandExecutor {
    timeout: Duration,
}

impl CommandExecutor {
    pub fn new(timeout: Duration) -> Self {
        Self { timeout }
    }

    pub async fn execute(
        &self,
        def: &CommandDef,
        params: Value,
        sandbox: Arc<SandboxContext>,
    ) -> ActResult<Value> {
        let name = def.name.as_str().to_string();
        let handler = def.handler.clone();
        match tokio::time::timeout(self.timeout, handler.execute(params, &sandbox)).await {
            Ok(result) => result,
            Err(_) => Err(ActError::Timeout(self.timeout)),
        }
        .map_err(|e| annotate(e, &name))
    }
}

fn annotate(err: ActError, command: &str) -> ActError {
    match err {
        ActError::Execution { detail, .. } => ActError::Execution {
            command: command.to_string(),
            detail,
        },
        other => other,
    }
}
