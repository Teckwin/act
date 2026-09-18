//! Command registry: definitions, naming enforcement, handler contract.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::context::SandboxContext;
use crate::error::{ActError, ActResult};
use crate::name::{CommandName, BUILTIN_DOMAINS};

/// Parameter verification semantics: PathLike/UrlLike params are routed
/// through the corresponding guard (path sandbox / URL policy) and are the
/// source for path_fields/url_fields derivation in the builder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "PascalCase")]
pub enum Verify {
    PathLike,
    UrlLike,
}

impl Verify {
    pub fn label(&self) -> &'static str {
        match self {
            Verify::PathLike => "PathLike",
            Verify::UrlLike => "UrlLike",
        }
    }
}

/// One declared output variant of a command: the success contract
/// (variant="done", success=true) or the envelope emitted for a specific
/// kernel error code (variant="failed_<code>", success=false).
#[derive(Debug, Clone, serde::Serialize)]
pub struct OutputSpec {
    pub variant: String,
    pub success: bool,
    pub schema: Value,
}

/// Capability class of a command; gates per-mode switches and protection level.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    /// Read-only filesystem access.
    Read,
    /// Mutating filesystem access (write/move/delete).
    Write,
    /// Network access.
    Net,
    /// Kernel meta commands (Sys_*): self-describing / self-managing.
    /// Exempt from read/write/net switches; still audited. Path/URL guards
    /// do not apply (no declared path/url fields) except where a handler
    /// explicitly resolves paths.
    Meta,
}

impl Capability {
    pub fn label(&self) -> &'static str {
        match self {
            Capability::Read => "read",
            Capability::Write => "write",
            Capability::Net => "net",
            Capability::Meta => "meta",
        }
    }
}

/// Extension contract: handlers only receive the sandbox context, never raw
/// filesystem or network freedom. All paths/urls must go through
/// `SandboxContext::resolve_path` / `check_url`.
#[async_trait]
pub trait CommandHandler: Send + Sync {
    async fn execute(&self, params: Value, ctx: &SandboxContext) -> ActResult<Value>;
}

/// A registered command definition.
#[derive(Clone)]
pub struct CommandDef {
    pub name: CommandName,
    pub description: String,
    pub capability: Capability,
    /// JSON schema of the parameters. Single source of truth: the CLI flag
    /// parser, the generated SKILL.md and schema.json all derive from it.
    pub param_schema: Value,
    /// Success output contract (the `done` variant schema).
    pub output_schema: Value,
    /// Declared output variants: `done` (success) + `failed_<code>` envelopes.
    pub outputs: Vec<OutputSpec>,
    /// Short command alias, e.g. `fr` for Fs_ReadFile.
    pub cmd_aliases: Vec<String>,
    /// CLI flag aliases: (flag name, canonical param name), e.g. ("p", "path").
    pub cli_aliases: Vec<(String, String)>,
    /// Canonical CLI usage example (flags only). Validated in tests so docs
    /// can never drift from the parser.
    pub example: Option<String>,
    /// JSON-pointer patterns locating path fields, e.g. `/paths/*`.
    pub path_fields: Vec<String>,
    /// JSON-pointer patterns locating url fields, e.g. `/urls/*`.
    pub url_fields: Vec<String>,
    pub handler: Arc<dyn CommandHandler>,
}

impl CommandDef {
    pub fn new(
        name: &str,
        description: impl Into<String>,
        capability: Capability,
        param_schema: Value,
        path_fields: Vec<String>,
        url_fields: Vec<String>,
        handler: Arc<dyn CommandHandler>,
    ) -> ActResult<Self> {
        Ok(Self {
            name: CommandName::parse(name)?,
            description: description.into(),
            capability,
            param_schema,
            output_schema: Value::Null,
            outputs: Vec::new(),
            cmd_aliases: Vec::new(),
            cli_aliases: Vec::new(),
            example: None,
            path_fields,
            url_fields,
            handler,
        })
    }
}

/// Serializable command metadata for `act list`, MCP `tools/list` and the
/// skill generator.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CommandInfo {
    pub name: String,
    pub description: String,
    pub capability: &'static str,
    pub input_schema: Value,
    pub output_schema: Value,
    pub outputs: Vec<OutputSpec>,
    pub cmd_aliases: Vec<String>,
    pub cli_aliases: Vec<(String, String)>,
    pub example: Option<String>,
    pub path_fields: Vec<String>,
    pub url_fields: Vec<String>,
}

#[derive(Default)]
pub struct CommandRegistry {
    defs: HashMap<String, CommandDef>,
    domains: Vec<String>,
}

impl CommandRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register an additional domain beyond the builtin ones (e.g. "Ext").
    pub fn register_domain(&mut self, domain: &str) -> ActResult<()> {
        let probe = format!("{domain}_Probe");
        let name = CommandName::parse(&probe)?;
        if name.domain() != domain {
            return Err(ActError::InvalidName {
                name: domain.to_string(),
            });
        }
        if !self.domains.iter().any(|d| d == domain) {
            self.domains.push(domain.to_string());
        }
        Ok(())
    }

    pub fn register(&mut self, def: CommandDef) -> ActResult<()> {
        def.name.ensure_domain_known(&self.domains)?;
        if self.defs.contains_key(def.name.as_str()) {
            return Err(ActError::DuplicateCommand {
                existing: def.name.as_str().to_string(),
            });
        }
        // Command aliases must not collide with any registered name/alias.
        for alias in &def.cmd_aliases {
            crate::builder::validate_alias(alias)?;
            if self.resolve(alias).is_some() {
                return Err(ActError::DuplicateCommand {
                    existing: format!("alias '{alias}'"),
                });
            }
        }
        self.defs.insert(def.name.as_str().to_string(), def);
        Ok(())
    }

    pub fn get(&self, name: &str) -> Option<&CommandDef> {
        self.defs.get(name)
    }

    /// Resolve a canonical name OR a short alias (`fr` → Fs_ReadFile).
    pub fn resolve(&self, token: &str) -> Option<&CommandDef> {
        if let Some(def) = self.defs.get(token) {
            return Some(def);
        }
        self.defs
            .values()
            .find(|def| def.cmd_aliases.iter().any(|a| a == token))
    }

    pub fn len(&self) -> usize {
        self.defs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.defs.is_empty()
    }

    /// Sorted metadata for listing surfaces.
    pub fn list(&self) -> Vec<CommandInfo> {
        let mut infos: Vec<CommandInfo> = self
            .defs
            .values()
            .map(|def| CommandInfo {
                name: def.name.as_str().to_string(),
                description: def.description.clone(),
                capability: def.capability.label(),
                input_schema: def.param_schema.clone(),
                output_schema: def.output_schema.clone(),
                outputs: def.outputs.clone(),
                cmd_aliases: def.cmd_aliases.clone(),
                cli_aliases: def.cli_aliases.clone(),
                example: def.example.clone(),
                path_fields: def.path_fields.clone(),
                url_fields: def.url_fields.clone(),
            })
            .collect();
        infos.sort_by(|a, b| a.name.cmp(&b.name));
        infos
    }

    pub fn domains(&self) -> Vec<String> {
        let mut all: Vec<String> = BUILTIN_DOMAINS.iter().map(|s| s.to_string()).collect();
        all.extend(self.domains.iter().cloned());
        all
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct Noop;
    #[async_trait]
    impl CommandHandler for Noop {
        async fn execute(&self, _params: Value, _ctx: &SandboxContext) -> ActResult<Value> {
            Ok(json!({}))
        }
    }

    fn def(name: &str) -> ActResult<CommandDef> {
        CommandDef::new(
            name,
            "test",
            Capability::Read,
            json!({}),
            vec![],
            vec![],
            Arc::new(Noop),
        )
    }

    #[test]
    fn valid_names_accepted() {
        def("Fs_ReadFile").expect("ok");
        def("Web_Search").expect("ok");
    }

    #[test]
    fn invalid_names_rejected() {
        assert!(def("fs_read").is_err());
        assert!(def("Fs_readFile").is_err());
        assert!(def("Fs_Read_File").is_err());
        assert!(def("Fs_").is_err());
        assert!(def("_ReadFile").is_err());
        assert!(def("Fs").is_err());
    }

    #[test]
    fn unknown_domain_rejected_until_registered() {
        let mut reg = CommandRegistry::new();
        let err = reg.register(def("Ext_DoSth").unwrap()).unwrap_err();
        assert!(matches!(err, ActError::UnknownDomain { .. }));
        reg.register_domain("Ext").unwrap();
        reg.register(def("Ext_DoSth").unwrap())
            .expect("ok after domain registered");
    }

    #[test]
    fn duplicate_rejected() {
        let mut reg = CommandRegistry::new();
        reg.register(def("Fs_ReadFile").unwrap()).unwrap();
        let err = reg.register(def("Fs_ReadFile").unwrap()).unwrap_err();
        assert!(matches!(err, ActError::DuplicateCommand { .. }));
    }

    #[test]
    fn list_is_sorted() {
        let mut reg = CommandRegistry::new();
        reg.register(def("Web_Search").unwrap()).unwrap();
        reg.register(def("Fs_ReadFile").unwrap()).unwrap();
        let names: Vec<String> = reg.list().into_iter().map(|c| c.name).collect();
        assert_eq!(names, vec!["Fs_ReadFile", "Web_Search"]);
    }
}
