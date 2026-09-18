//! Commander-style registration API (npm commander.js inspired):
//!
//! ```ignore
//! CommandBuilder::new("Fs_ReadFile", "Read one file…", Capability::Read, "fr")
//!     .param(Param::string("path").alias("p").required().verify(Verify::PathLike))
//!     .param(Param::integer("offset").alias("o").min(0))
//!     .output_done(json!({ "type": "object", "required": ["ok","path","content"], ... }))
//!     .output_fail("permission_denied", json!({ ...envelope for this code... }))
//!     .example("--path src/main.rs")
//!     .bind(Arc::new(ReadFile))
//! ```
//!
//! Registration IS the contract: the builder derives the JSON-schema
//! (inputSchema), path/url field pointers (from `verify`), CLI aliases and
//! output variants (success + per-error envelopes). The kernel validates
//! handler results against the success contract at runtime.

use serde_json::{json, Value};

use crate::error::{ActError, ActResult};
use crate::name::CommandName;
pub use crate::registry::{Capability, CommandDef, CommandHandler, OutputSpec, Verify};

/// Reserved CLI subcommand names that can never be command aliases.
pub const RESERVED_ALIASES: &[&str] = &["help"];

/// Validate a short command alias (`fr`, `grep`, …).
pub fn validate_alias(alias: &str) -> ActResult<()> {
    let ok_len = (2..=8).contains(&alias.len());
    let ok_chars = alias
        .chars()
        .next()
        .map(|c| c.is_ascii_lowercase())
        .unwrap_or(false)
        && alias
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    if !ok_len || !ok_chars {
        return Err(ActError::InvalidName {
            name: format!("alias '{alias}' (expected 2-8 lowercase alphanumeric, leading letter)"),
        });
    }
    if RESERVED_ALIASES.contains(&alias) {
        return Err(ActError::InvalidName {
            name: format!("alias '{alias}' (reserved builtin subcommand)"),
        });
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub enum ParamType {
    String,
    Integer,
    Number,
    Boolean,
    Array { item: Box<ParamType> },
    Object,
}

impl ParamType {
    fn schema_type(&self) -> Value {
        match self {
            ParamType::String => json!("string"),
            ParamType::Integer => json!("integer"),
            ParamType::Number => json!("number"),
            ParamType::Boolean => json!("boolean"),
            ParamType::Object => json!("object"),
            ParamType::Array { item } => {
                json!({ "type": "array", "items": { "type": item.schema_type() } })
            }
        }
    }
}

/// Fluent parameter spec.
#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub typ: ParamType,
    pub aliases: Vec<String>,
    pub required: bool,
    pub verify: Option<Verify>,
    pub default: Option<Value>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub enum_values: Option<Vec<String>>,
    pub description: Option<String>,
}

/// Emit whole numbers as integers (100.0 → 100) for clean schemas.
fn number_json(v: f64) -> Value {
    if v.fract() == 0.0 && v.abs() <= i64::MAX as f64 {
        json!(v as i64)
    } else {
        json!(v)
    }
}

impl Param {
    pub fn string(name: &str) -> Self {
        Self::new(name, ParamType::String)
    }
    pub fn integer(name: &str) -> Self {
        Self::new(name, ParamType::Integer)
    }
    pub fn number(name: &str) -> Self {
        Self::new(name, ParamType::Number)
    }
    pub fn boolean(name: &str) -> Self {
        Self::new(name, ParamType::Boolean)
    }
    pub fn array_of_string(name: &str) -> Self {
        Self::new(
            name,
            ParamType::Array {
                item: Box::new(ParamType::String),
            },
        )
    }
    pub fn array_of_integer(name: &str) -> Self {
        Self::new(
            name,
            ParamType::Array {
                item: Box::new(ParamType::Integer),
            },
        )
    }
    pub fn array_of_object(name: &str) -> Self {
        Self::new(
            name,
            ParamType::Array {
                item: Box::new(ParamType::Object),
            },
        )
    }
    pub fn object(name: &str) -> Self {
        Self::new(name, ParamType::Object)
    }

    fn new(name: &str, typ: ParamType) -> Self {
        Self {
            name: name.to_string(),
            typ,
            aliases: Vec::new(),
            required: false,
            verify: None,
            default: None,
            min: None,
            max: None,
            enum_values: None,
            description: None,
        }
    }

    pub fn alias(mut self, short: &str) -> Self {
        self.aliases.push(short.to_string());
        self
    }
    pub fn required(mut self) -> Self {
        self.required = true;
        self
    }
    pub fn verify(mut self, verify: Verify) -> Self {
        self.verify = Some(verify);
        self
    }
    pub fn default(mut self, value: Value) -> Self {
        self.default = Some(value);
        self
    }
    pub fn min(mut self, v: f64) -> Self {
        self.min = Some(v);
        self
    }
    pub fn max(mut self, v: f64) -> Self {
        self.max = Some(v);
        self
    }
    pub fn enum_values(mut self, values: &[&str]) -> Self {
        self.enum_values = Some(values.iter().map(|s| s.to_string()).collect());
        self
    }
    pub fn desc(mut self, text: &str) -> Self {
        self.description = Some(text.to_string());
        self
    }

    /// Render into a JSON-schema property.
    pub fn to_schema_property(&self) -> Value {
        let mut prop = match &self.typ {
            ParamType::Array { .. } => self.typ.schema_type(),
            other => json!({ "type": other.schema_type() }),
        };
        let obj = prop.as_object_mut().expect("property is object");
        if let Some(values) = &self.enum_values {
            obj.insert("enum".into(), json!(values));
        }
        if let Some(min) = self.min {
            let key = if matches!(self.typ, ParamType::Integer | ParamType::Number) {
                "minimum"
            } else {
                "minItems"
            };
            obj.insert(key.into(), number_json(min));
        }
        if let Some(max) = self.max {
            let key = if matches!(self.typ, ParamType::Integer | ParamType::Number) {
                "maximum"
            } else {
                "maxItems"
            };
            obj.insert(key.into(), number_json(max));
        }
        if let Some(default) = &self.default {
            obj.insert("default".into(), default.clone());
        }
        if let Some(description) = &self.description {
            obj.insert("description".into(), json!(description));
        }
        if let Some(verify) = &self.verify {
            obj.insert("x-verify".into(), json!(verify.label()));
        }
        prop
    }
}

/// Fluent command builder — the single registration entry point.
pub struct CommandBuilder {
    name: CommandName,
    description: String,
    capability: Capability,
    alias: Option<String>,
    params: Vec<Param>,
    outputs: Vec<OutputSpec>,
    example: Option<String>,
}

impl CommandBuilder {
    pub fn new(
        name: &str,
        description: impl Into<String>,
        capability: Capability,
        short_alias: &str,
    ) -> Self {
        Self {
            name: CommandName::parse(name).expect("builder command name"),
            description: description.into(),
            capability,
            alias: (!short_alias.is_empty()).then(|| short_alias.to_string()),
            params: Vec::new(),
            outputs: Vec::new(),
            example: None,
        }
    }

    pub fn param(mut self, param: Param) -> Self {
        self.params.push(param);
        self
    }

    /// Success output contract (`variant="done"`, ok=true).
    pub fn output_done(mut self, schema: Value) -> Self {
        self.outputs.push(OutputSpec {
            variant: "done".into(),
            success: true,
            schema,
        });
        self
    }

    /// Failure output contract for a kernel error code
    /// (`failed_<code>`, ok=false) — documents the emitted envelope.
    pub fn output_fail(mut self, error_code: &str, schema: Value) -> Self {
        self.outputs.push(OutputSpec {
            variant: format!("failed_{error_code}"),
            success: false,
            schema,
        });
        self
    }

    pub fn example(mut self, example: &str) -> Self {
        self.example = Some(example.to_string());
        self
    }

    /// Finish the definition and bind the handler.
    pub fn bind(self, handler: std::sync::Arc<dyn CommandHandler>) -> ActResult<CommandDef> {
        if let Some(alias) = &self.alias {
            validate_alias(alias)?;
        }
        if self.params.iter().filter(|p| p.required).count() == 0
            && !self.params.is_empty()
            && self.name.domain() != "Sys"
        {
            tracing::debug!("command {} has no required params", self.name);
        }
        // Duplicate param names / alias collisions inside one command.
        let mut seen = std::collections::HashSet::new();
        for param in &self.params {
            if !seen.insert(param.name.clone()) {
                return Err(ActError::InvalidParams {
                    command: self.name.as_str().to_string(),
                    detail: format!("duplicate param '{}'", param.name),
                });
            }
            for alias in &param.aliases {
                if !seen.insert(format!(":{alias}")) {
                    return Err(ActError::InvalidParams {
                        command: self.name.as_str().to_string(),
                        detail: format!("param alias collision '{}'", alias),
                    });
                }
            }
        }
        if self.outputs.iter().any(|o| o.success) == false {
            return Err(ActError::InvalidParams {
                command: self.name.as_str().to_string(),
                detail: "exactly one success output ('done') required".into(),
            });
        }

        // Derive the input schema from the param specs.
        let mut properties = serde_json::Map::new();
        let mut required = Vec::new();
        let mut path_fields = Vec::new();
        let mut url_fields = Vec::new();
        let mut cli_aliases = Vec::new();
        for param in &self.params {
            let is_array = matches!(param.typ, ParamType::Array { .. });
            properties.insert(param.name.clone(), param.to_schema_property());
            if param.required {
                required.push(json!(param.name));
            }
            for alias in &param.aliases {
                cli_aliases.push((alias.clone(), param.name.clone()));
            }
            match &param.verify {
                Some(Verify::PathLike) => {
                    path_fields.push(if is_array {
                        format!("/{}/*", param.name)
                    } else {
                        format!("/{}", param.name)
                    });
                }
                Some(Verify::UrlLike) => {
                    url_fields.push(if is_array {
                        format!("/{}/*", param.name)
                    } else {
                        format!("/{}", param.name)
                    });
                }
                None => {}
            }
        }

        let output_schema = self
            .outputs
            .iter()
            .find(|o| o.success)
            .map(|o| o.schema.clone())
            .unwrap_or(Value::Null);

        Ok(CommandDef {
            name: self.name,
            description: self.description,
            capability: self.capability,
            param_schema: json!({
                "type": "object",
                "properties": Value::Object(properties),
                "required": required,
            }),
            output_schema,
            cli_aliases,
            cmd_aliases: self.alias.into_iter().collect(),
            outputs: self.outputs,
            example: self.example,
            path_fields,
            url_fields,
            handler,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::SandboxContext;
    use async_trait::async_trait;

    struct Noop;
    #[async_trait]
    impl CommandHandler for Noop {
        async fn execute(&self, _params: Value, _ctx: &SandboxContext) -> ActResult<Value> {
            Ok(json!({ "ok": true }))
        }
    }

    fn expect_err(r: ActResult<CommandDef>) -> ActError {
        match r {
            Err(e) => e,
            Ok(_) => panic!("expected registration error"),
        }
    }
    fn builder() -> CommandBuilder {
        CommandBuilder::new("Fs_Demo", "demo", Capability::Read, "fd")
            .param(
                Param::string("path")
                    .alias("p")
                    .required()
                    .verify(Verify::PathLike)
                    .desc("a path"),
            )
            .param(
                Param::integer("limit")
                    .alias("l")
                    .min(1.0)
                    .max(100.0)
                    .default(json!(10)),
            )
            .param(Param::boolean("hidden").default(json!(false)))
            .output_done(
                json!({ "type": "object", "required": ["ok", "path"], "properties": {
                "ok": { "type": "boolean" }, "path": { "type": "string" } } }),
            )
            .example("--path a.txt")
    }

    #[test]
    fn derives_schema_and_fields() {
        let def = builder().bind(std::sync::Arc::new(Noop)).unwrap();
        let props = def.param_schema["properties"].as_object().unwrap();
        assert!(props.contains_key("path"));
        assert_eq!(def.param_schema["required"][0], json!("path"));
        assert_eq!(props["limit"]["maximum"], json!(100));
        assert_eq!(props["limit"]["default"], json!(10));
        assert_eq!(props["path"]["x-verify"], json!("PathLike"));
        // path_fields derived from verify
        assert_eq!(def.path_fields, vec!["/path".to_string()]);
        // cli_aliases derived from param aliases
        assert!(def
            .cli_aliases
            .contains(&("l".to_string(), "limit".to_string())));
        assert_eq!(def.cmd_aliases, vec!["fd".to_string()]);
        // success output contract present
        assert!(def.outputs.iter().any(|o| o.success));
    }

    #[test]
    fn array_verify_yields_wildcard_pointer() {
        let def = CommandBuilder::new("Fs_Demo", "d", Capability::Write, "fdm")
            .param(
                Param::array_of_string("paths")
                    .alias("ps")
                    .required()
                    .verify(Verify::PathLike),
            )
            .output_done(json!({ "type": "object" }))
            .bind(std::sync::Arc::new(Noop))
            .unwrap();
        assert_eq!(def.path_fields, vec!["/paths/*".to_string()]);
    }

    #[test]
    fn reserved_and_bad_aliases_rejected() {
        assert!(validate_alias("help").is_err());
        assert!(validate_alias("X").is_err());
        assert!(validate_alias("toolongalias9").is_err());
        assert!(validate_alias("fr").is_ok());
        assert!(validate_alias("mcp").is_ok());

        let err = expect_err(
            CommandBuilder::new("Fs_Demo", "d", Capability::Read, "help")
                .param(Param::string("path").required())
                .output_done(json!({}))
                .bind(std::sync::Arc::new(Noop)),
        );
        assert!(matches!(err, ActError::InvalidName { .. }));
    }

    #[test]
    fn duplicate_param_and_alias_rejected() {
        let err = expect_err(
            CommandBuilder::new("Fs_Demo", "d", Capability::Read, "fd")
                .param(Param::string("path").alias("p").required())
                .param(Param::string("other").alias("p"))
                .output_done(json!({}))
                .bind(std::sync::Arc::new(Noop)),
        );
        assert!(err.to_string().contains("collision"));

        let err = expect_err(
            CommandBuilder::new("Fs_Demo", "d", Capability::Read, "fd")
                .param(Param::string("path").required())
                .param(Param::string("path"))
                .output_done(json!({}))
                .bind(std::sync::Arc::new(Noop)),
        );
        assert!(err.to_string().contains("duplicate"));
    }

    #[test]
    fn success_output_mandatory() {
        let err = expect_err(
            CommandBuilder::new("Fs_Demo", "d", Capability::Read, "fd")
                .param(Param::string("path").required())
                .bind(std::sync::Arc::new(Noop)),
        );
        assert!(err.to_string().contains("success output"));
    }
}
