//! CommandManager: the single entry point tying registry, verifier, executor,
//! output policy and audit together. No other path can reach a handler.

use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use serde_json::Value;

use crate::audit::{AuditEntry, AuditLog};
use crate::config::ActConfig;
use crate::context::{InvokeMode, SandboxContext};
use crate::error::{ActError, ActResult};
use crate::executor::CommandExecutor;
use crate::guard::{
    path_guard::PathGuard, protect_guard::ProtectGuard, url_guard::UrlGuard, PermissionVerifier,
    Verification,
};
use crate::output::OutputPolicy;
use crate::registry::{CommandDef, CommandInfo, CommandRegistry};

pub struct CommandManager {
    config: Arc<ActConfig>,
    registry: RwLock<CommandRegistry>,
    verifier: PermissionVerifier,
    executor: CommandExecutor,
    output: OutputPolicy,
    audit: AuditLog,
    sandbox: Arc<SandboxContext>,
}

impl CommandManager {
    pub fn new(config: ActConfig) -> ActResult<Self> {
        let path_guard = Arc::new(PathGuard::new(config.roots.clone())?);
        let url_guard = UrlGuard::new(config.url.clone());
        let protect_guard =
            ProtectGuard::from_config(&config.protected, config.allow_read_protected);
        let verifier = PermissionVerifier::new(&config)?;
        let timeout = Duration::from_secs(config.limits.command_timeout_secs);
        let primary_root = path_guard.primary_root().to_path_buf();
        let audit = AuditLog::new(primary_root.join(".act/audit.jsonl"))
            .map_err(|e| ActError::Config(format!("cannot create audit log: {e}")))?;
        let sandbox = Arc::new(SandboxContext::new(
            Arc::new(config.clone()),
            path_guard,
            url_guard,
            protect_guard,
        ));
        Ok(Self {
            config: Arc::new(config),
            registry: RwLock::new(CommandRegistry::new()),
            verifier,
            executor: CommandExecutor::new(timeout),
            output: OutputPolicy::new(),
            audit,
            sandbox,
        })
    }

    pub fn config(&self) -> &ActConfig {
        &self.config
    }

    pub fn sandbox(&self) -> &Arc<SandboxContext> {
        &self.sandbox
    }

    pub fn register_domain(&self, domain: &str) -> ActResult<()> {
        self.registry.write().register_domain(domain)
    }

    pub fn register(&self, def: CommandDef) -> ActResult<()> {
        self.registry.write().register(def)
    }

    pub fn list(&self) -> Vec<CommandInfo> {
        self.registry.read().list()
    }

    /// Direct access to a command definition (tests / generators).
    pub fn registry_def(&self, name: &str) -> Option<CommandDef> {
        self.registry.read().get(name).cloned()
    }

    /// Resolve a canonical name OR a short alias to its definition.
    pub fn resolve_def(&self, token: &str) -> Option<CommandDef> {
        self.registry.read().resolve(token).cloned()
    }

    /// Permission dry-run: verify without executing.
    pub async fn verify_only(
        &self,
        name: &str,
        params: &Value,
        mode: InvokeMode,
    ) -> ActResult<Verification> {
        let def = self
            .registry
            .read()
            .get(name)
            .cloned()
            .ok_or_else(|| ActError::UnknownCommand(name.to_string()))?;
        self.verifier.verify(&def, params, &mode).await
    }

    /// Execute a command by name. This is the only route to any handler.
    pub async fn execute(&self, name: &str, params: Value, mode: InvokeMode) -> ActResult<Value> {
        let started = Instant::now();
        let def = self
            .registry
            .read()
            .get(name)
            .cloned()
            .ok_or_else(|| ActError::UnknownCommand(name.to_string()))?;

        // Extract path/url fields for the audit trail.
        let (audit_paths, audit_urls) = extract_audit_fields(&def, &params);

        // Permission pipeline.
        let verification = match self.verifier.verify(&def, &params, &mode).await {
            Ok(v) => v,
            Err(err) => {
                self.audit_record(&mode, name, audit_paths, audit_urls, false, &err, started);
                return Err(err);
            }
        };
        let _ = verification;

        // Execute with timeout.
        let exec_result = self
            .executor
            .execute(&def, params, self.sandbox.clone())
            .await;
        match exec_result {
            Ok(result) => {
                // Runtime output-contract check against the declared success
                // schema (registration-time contract; see CommandBuilder).
                if let Err(err) = Self::validate_output(&def, &result) {
                    self.audit_record(
                        &mode,
                        name,
                        audit_paths.clone(),
                        audit_urls.clone(),
                        false,
                        &err,
                        started,
                    );
                    return Err(err);
                }
                // Output policy (overflow + summary). Meta commands
                // (Sys_Schema/Sys_List --json …) emit the contract document
                // itself and must never be compressed.
                let final_result = if def.capability == crate::registry::Capability::Meta {
                    result
                } else {
                    self.output
                        .apply(name, result, &self.sandbox)
                        .await
                        .map_err(|err| {
                            self.audit_record(
                                &mode,
                                name,
                                audit_paths.clone(),
                                audit_urls.clone(),
                                false,
                                &err,
                                started,
                            );
                            err
                        })?
                };
                self.audit_record(
                    &mode,
                    name,
                    audit_paths,
                    audit_urls,
                    true,
                    &ActError::Other(String::new()),
                    started,
                );
                Ok(final_result)
            }
            Err(err) => {
                self.audit_record(&mode, name, audit_paths, audit_urls, false, &err, started);
                Err(err)
            }
        }
    }

    fn audit_record(
        &self,
        mode: &InvokeMode,
        command: &str,
        paths: Vec<String>,
        urls: Vec<String>,
        allowed: bool,
        err: &ActError,
        started: Instant,
    ) {
        let (error_code, error) = if allowed {
            (None, None)
        } else {
            (Some(err.code().to_string()), Some(err.to_string()))
        };
        self.audit.record(AuditEntry {
            ts: chrono::Utc::now(),
            mode: mode.label().to_string(),
            command: command.to_string(),
            paths,
            urls,
            allowed,
            error_code,
            error,
            duration_ms: started.elapsed().as_millis() as u64,
        });
    }
}

impl CommandManager {
    /// Validate a successful handler result against the command's declared
    /// `done` output contract: required fields present + top-level types
    /// match. Keeps handlers honest and enables mock/black-box contract tests.
    fn validate_output(def: &CommandDef, result: &Value) -> ActResult<()> {
        let Some(contract) = def.outputs.iter().find(|o| o.success) else {
            return Ok(()); // legacy definitions without declared outputs
        };
        let command = def.name.as_str();
        let Some(obj) = result.as_object() else {
            return Err(ActError::output_contract(
                command,
                "success result must be a JSON object",
            ));
        };
        let empty = Vec::new();
        let required = contract
            .schema
            .get("required")
            .and_then(|r| r.as_array())
            .unwrap_or(&empty);
        for field in required {
            let Some(field) = field.as_str() else {
                continue;
            };
            if !obj.contains_key(field) {
                return Err(ActError::output_contract(
                    command,
                    format!("missing required output field '{field}'"),
                ));
            }
        }
        if let Some(props) = contract
            .schema
            .get("properties")
            .and_then(|p| p.as_object())
        {
            for (field, spec) in props {
                let Some(value) = obj.get(field) else {
                    continue;
                };
                if let Some(expected) = spec.get("type").and_then(|t| t.as_str()) {
                    let actual = json_type_name(value);
                    if actual != expected {
                        return Err(ActError::output_contract(
                            command,
                            format!(
                                "output field '{field}' expected type '{expected}', got '{actual}'"
                            ),
                        ));
                    }
                }
                if let Some(constant) = spec.get("const") {
                    if value != constant {
                        return Err(ActError::output_contract(
                            command,
                            format!("output field '{field}' must equal {constant}"),
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

fn json_type_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(n) => {
            if n.is_i64() || n.is_u64() {
                "integer"
            } else {
                "number"
            }
        }
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn extract_audit_fields(def: &CommandDef, params: &Value) -> (Vec<String>, Vec<String>) {
    let mut paths = Vec::new();
    for field in &def.path_fields {
        paths.extend(crate::param::extract_strings(params, field));
    }
    let mut urls = Vec::new();
    for field in &def.url_fields {
        urls.extend(crate::param::extract_strings(params, field));
    }
    (paths, urls)
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use serde_json::json;

    struct Echo;
    #[async_trait]
    impl crate::registry::CommandHandler for Echo {
        async fn execute(&self, params: Value, _ctx: &SandboxContext) -> ActResult<Value> {
            Ok(params)
        }
    }

    fn manager_in(dir: &std::path::Path) -> CommandManager {
        let cfg = ActConfig {
            roots: vec![dir.to_path_buf()],
            ..Default::default()
        };
        CommandManager::new(cfg).expect("manager")
    }

    #[tokio::test]
    async fn happy_path_and_audit() {
        let tmp = tempfile::tempdir().unwrap();
        let m = manager_in(tmp.path());
        m.register(
            CommandDef::new(
                "Fs_ReadFile",
                "echo test",
                crate::registry::Capability::Read,
                json!({}),
                vec!["/paths/*".into()],
                vec![],
                Arc::new(Echo),
            )
            .unwrap(),
        )
        .unwrap();

        std::fs::write(tmp.path().join("a.txt"), b"x").unwrap();
        let out = m
            .execute("Fs_ReadFile", json!({"paths": ["a.txt"]}), InvokeMode::Cli)
            .await
            .unwrap();
        assert_eq!(out["paths"][0], "a.txt");

        let audit = std::fs::read_to_string(tmp.path().join(".act/audit.jsonl")).unwrap();
        assert!(audit.contains("\"command\":\"Fs_ReadFile\""));
        assert!(audit.contains("\"allowed\":true"));
    }

    #[tokio::test]
    async fn unknown_command_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let m = manager_in(tmp.path());
        let err = m
            .execute("Nope_Nothing", json!({}), InvokeMode::Cli)
            .await
            .unwrap_err();
        assert!(matches!(err, ActError::UnknownCommand(_)));
    }

    #[tokio::test]
    async fn traversal_denied_and_audited() {
        let tmp = tempfile::tempdir().unwrap();
        let m = manager_in(tmp.path());
        m.register(
            CommandDef::new(
                "Fs_ReadFile",
                "echo test",
                crate::registry::Capability::Read,
                json!({}),
                vec!["/paths/*".into()],
                vec![],
                Arc::new(Echo),
            )
            .unwrap(),
        )
        .unwrap();

        let err = m
            .execute(
                "Fs_ReadFile",
                json!({"paths": ["../../outside.txt"]}),
                InvokeMode::Mcp,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ActError::PermissionDenied { .. }));
        assert_eq!(err.exit_code(), 2);
        let audit = std::fs::read_to_string(tmp.path().join(".act/audit.jsonl")).unwrap();
        assert!(audit.contains("\"allowed\":false"));
        assert!(audit.contains("permission_denied"));
    }

    #[tokio::test]
    async fn protected_path_denied() {
        let tmp = tempfile::tempdir().unwrap();
        let m = manager_in(tmp.path());
        m.register(
            CommandDef::new(
                "Fs_WriteFile",
                "echo test",
                crate::registry::Capability::Write,
                json!({}),
                vec!["/files/*/path".into()],
                vec![],
                Arc::new(Echo),
            )
            .unwrap(),
        )
        .unwrap();

        let err = m
            .execute(
                "Fs_WriteFile",
                json!({"files": [{"path": ".git/config", "content": "x"}]}),
                InvokeMode::Cli,
            )
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            ActError::PermissionDenied {
                guard: "ProtectGuard",
                ..
            }
        ));
    }
}
