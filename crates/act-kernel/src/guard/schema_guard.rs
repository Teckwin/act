//! SchemaGuard: kernel-side schema-conformance validation.
//!
//! SINGLE RESPONSIBILITY of the kernel: enforce the registration-time
//! contract (`required` / `type` / `enum` / `minimum` / `maximum` /
//! `minItems` / `maxItems`) for EVERY channel — CLI parser, MCP tools/call
//! JSON, or direct `exec` API. The CLI parser performs normalization only;
//! this guard is the authoritative validation layer.

use serde_json::Value;

use crate::error::{ActError, ActResult};
use crate::registry::CommandDef;

/// Validate params against the command's declared input schema
/// (top-level properties only — handlers own nested semantics).
pub fn validate(def: &CommandDef, params: &Value) -> ActResult<()> {
    let command = def.name.as_str();
    let obj = params
        .as_object()
        .ok_or_else(|| ActError::invalid_params(command, "params must be a JSON object"))?;
    let schema = &def.param_schema;
    let empty_props = serde_json::Map::new();
    let props = schema
        .get("properties")
        .and_then(|p| p.as_object())
        .unwrap_or(&empty_props);

    // Unknown top-level params are rejected (strict contract). The CLI
    // parser's internal key (`target_params` for Sys_Verify) is allowed.
    for key in obj.keys() {
        if !props.contains_key(key) && key != "target_params" {
            return Err(ActError::invalid_params(
                command,
                format!("unknown parameter '{key}'"),
            ));
        }
    }

    // Required fields.
    if let Some(required) = schema.get("required").and_then(|r| r.as_array()) {
        let missing: Vec<&str> = required
            .iter()
            .filter_map(|r| r.as_str())
            .filter(|name| !obj.contains_key(*name))
            .collect();
        if !missing.is_empty() {
            return Err(ActError::invalid_params(
                command,
                format!(
                    "missing required parameter(s): {}",
                    missing
                        .iter()
                        .map(|m| format!("--{}", m.replace('_', "-")))
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }
    }

    // Per-property constraints.
    for (name, prop) in props {
        let Some(value) = obj.get(name) else { continue };
        check_property(command, name, prop, value)?;
    }
    Ok(())
}

fn check_property(command: &str, name: &str, prop: &Value, value: &Value) -> ActResult<()> {
    if let Some(expected) = prop.get("type").and_then(|t| t.as_str()) {
        let actual = json_type_name(value);
        let ok = match (expected, actual) {
            ("integer", "integer") => true,
            ("number", "number") | ("number", "integer") => true,
            (a, b) => a == b,
        };
        if !ok {
            return Err(ActError::invalid_params(
                command,
                format!("parameter '{name}' expects type '{expected}', got '{actual}'"),
            ));
        }
    }
    if let Some(allowed) = prop.get("enum").and_then(|e| e.as_array()) {
        if !allowed.contains(value) {
            return Err(ActError::invalid_params(
                command,
                format!("parameter '{name}' must be one of {allowed:?}, got {value}"),
            ));
        }
    }
    if let Some(min) = prop.get("minimum").and_then(|v| v.as_f64()) {
        let ok = value.as_f64().map(|n| n >= min).unwrap_or(true);
        if !ok {
            return Err(ActError::invalid_params(
                command,
                format!("parameter '{name}' must be >= {min}"),
            ));
        }
    }
    if let Some(max) = prop.get("maximum").and_then(|v| v.as_f64()) {
        let ok = value.as_f64().map(|n| n <= max).unwrap_or(true);
        if !ok {
            return Err(ActError::invalid_params(
                command,
                format!("parameter '{name}' must be <= {max}"),
            ));
        }
    }
    if let Some(min_items) = prop.get("minItems").and_then(|v| v.as_u64()) {
        let ok = value
            .as_array()
            .map(|a| a.len() as u64 >= min_items)
            .unwrap_or(true);
        if !ok {
            return Err(ActError::invalid_params(
                command,
                format!("parameter '{name}' needs at least {min_items} item(s)"),
            ));
        }
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn def_with_schema() -> CommandDef {
        crate::builder::CommandBuilder::new(
            "Fs_Demo",
            "demo",
            crate::registry::Capability::Read,
            "fd",
        )
        .param(
            crate::builder::Param::string("path")
                .alias("p")
                .required()
                .verify(crate::registry::Verify::PathLike),
        )
        .param(
            crate::builder::Param::integer("limit")
                .alias("l")
                .min(1.0)
                .max(100.0)
                .default(json!(10)),
        )
        .param(crate::builder::Param::string("mode").enum_values(&["a", "b"]))
        .output_done(json!({ "type": "object" }))
        .bind(std::sync::Arc::new(Noop))
        .expect("def")
    }

    struct Noop;
    #[async_trait::async_trait]
    impl crate::registry::CommandHandler for Noop {
        async fn execute(
            &self,
            _p: Value,
            _c: &crate::context::SandboxContext,
        ) -> ActResult<Value> {
            Ok(json!({ "ok": true }))
        }
    }

    #[test]
    fn accepts_conforming_params() {
        let def = def_with_schema();
        validate(&def, &json!({"path": "a.rs", "limit": 5, "mode": "a"})).unwrap();
        // defaults are optional
        validate(&def, &json!({"path": "a.rs"})).unwrap();
    }

    #[test]
    fn missing_required_rejected() {
        let def = def_with_schema();
        let err = validate(&def, &json!({"limit": 5})).unwrap_err();
        assert!(err.to_string().contains("--path"));
    }

    #[test]
    fn type_mismatch_rejected() {
        let def = def_with_schema();
        let err = validate(&def, &json!({"path": "a", "limit": "abc"})).unwrap_err();
        assert!(err.to_string().contains("expects type 'integer'"));
    }

    #[test]
    fn range_and_enum_rejected() {
        let def = def_with_schema();
        assert!(validate(&def, &json!({"path": "a", "limit": 0})).is_err());
        assert!(validate(&def, &json!({"path": "a", "limit": 101})).is_err());
        assert!(validate(&def, &json!({"path": "a", "mode": "c"})).is_err());
    }

    #[test]
    fn unknown_param_rejected() {
        let def = def_with_schema();
        let err = validate(&def, &json!({"path": "a", "bogus": 1})).unwrap_err();
        assert!(err.to_string().contains("unknown parameter"));
    }

    #[test]
    fn target_params_allowed_for_verify() {
        // Sys_Verify's internal key passes through.
        let def = def_with_schema();
        validate(&def, &json!({"path": "a", "target_params": {"x": 1}})).unwrap();
    }
}
