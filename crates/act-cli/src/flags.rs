//! Schema-driven CLI flag parser: turns `act Fs_ReadFile --path a.rs --limit 10`
//! into the JSON params consumed by the kernel.
//!
//! The param schema in each CommandDef is the single source of truth:
//! - `string`  → `--name value`
//! - `integer` / `number` → `--name 42` (bad numbers rejected)
//! - `boolean` → `--name` (=true) / `--name=false` / `--no-name` (=false)
//! - `array of string/integer` → `--name a,b,c` (comma split) and/or repeatable
//! - `array of object` / `object` → `--name '{json}'`; `--edit "old=>new"`
//!   sugar is repeatable (Fs_EditFile)
//! Unknown flags and missing required fields are rejected with invalid_params.

use act_kernel::error::{ActError, ActResult};
use act_kernel::{CommandDef, CommandInfo};
use serde_json::{json, Map, Value};

pub fn is_builtin_subcommand(token: &str) -> bool {
    matches!(
        token,
        "exec" | "list" | "verify" | "mcp" | "schema" | "package" | "install" | "help"
    )
}

/// Parse flag arguments into a JSON params object for the given command.
pub fn parse(info: &CommandInfo, args: &[String]) -> ActResult<Value> {
    let props = info
        .input_schema
        .get("properties")
        .and_then(|v| v.as_object())
        .ok_or_else(|| {
            ActError::Other(format!("command {} has no schema properties", info.name))
        })?;
    let aliases: Vec<(String, String)> = info
        .cli_aliases
        .iter()
        .map(|(a, b)| (normalize_flag(a), b.clone()))
        .collect();

    let mut out = Map::new();
    let mut i = 0usize;
    while i < args.len() {
        let token = &args[i];
        // `--flag` / `--flag=v` (canonical) or `-p` / `-p=v` (short alias).
        let stripped = if let Some(rest) = token.strip_prefix("--") {
            rest
        } else if let Some(rest) = token.strip_prefix('-') {
            if rest.is_empty() || rest.starts_with(|c: char| c.is_ascii_digit()) {
                return Err(ActError::invalid_params(
                    &info.name,
                    format!("unexpected positional argument '{token}'; use --flags (JSON channel: act exec)"),
                ));
            }
            rest
        } else {
            return Err(ActError::invalid_params(
                &info.name,
                format!("unexpected positional argument '{token}'; use --flags (JSON channel: act exec)"),
            ));
        };
        let (flag_name, inline_value) = match stripped.split_once('=') {
            Some((n, v)) => (normalize_flag(n), Some(v.to_string())),
            None => (normalize_flag(stripped), None),
        };
        if stripped == "help" {
            return Err(ActError::invalid_params(
                &info.name,
                format!(
                    "usage: act {} {}",
                    info.name,
                    info.example.as_deref().unwrap_or("--<params>")
                ),
            ));
        }

        // Resolve alias → canonical property name.
        let (canonical, negated) = resolve(&flag_name, props, &aliases, &info.name)?;
        let schema_prop = props.get(&canonical).ok_or_else(|| {
            ActError::invalid_params(&info.name, format!("unknown flag --{flag_name}"))
        })?;
        let prop_type = schema_prop
            .get("type")
            .and_then(|t| t.as_str())
            .unwrap_or("string");

        let (value, consumed_next) = match prop_type {
            "boolean" => {
                let v = if negated {
                    false
                } else {
                    match inline_value.as_deref() {
                        None => true,
                        Some("true") => true,
                        Some("false") => false,
                        Some(other) => {
                            return Err(ActError::invalid_params(
                                &info.name,
                                format!("--{flag_name} expects true/false, got '{other}'"),
                            ));
                        }
                    }
                };
                (Value::Bool(v), false)
            }
            _ => {
                let raw = match inline_value.clone() {
                    Some(v) => v,
                    None => {
                        i += 1;
                        if i >= args.len() {
                            return Err(ActError::invalid_params(
                                &info.name,
                                format!("--{flag_name} expects a value"),
                            ));
                        }
                        args[i].clone()
                    }
                };
                (
                    convert(
                        &info.name,
                        &flag_name,
                        &canonical,
                        prop_type,
                        &raw,
                        schema_prop,
                    )?,
                    true,
                )
            }
        };

        merge(
            &mut out, &canonical, value, prop_type, &info.name, &flag_name,
        )?;
        let _ = consumed_next;
        i += 1;
    }

    // Fs_EditFile sugar: top-level replace_all applies to every edit that did
    // not set it explicitly.
    if info.name == "Fs_EditFile" {
        if out.get("replace_all").and_then(|v| v.as_bool()) == Some(true) {
            if let Some(Value::Array(edits)) = out.get_mut("edits") {
                for edit in edits.iter_mut() {
                    if edit.get("replace_all").is_none() {
                        edit["replace_all"] = json!(true);
                    }
                }
            }
        }
    }

    // Required-field validation (fail fast with a helpful message).
    if let Some(required) = info.input_schema.get("required").and_then(|r| r.as_array()) {
        let missing: Vec<String> = required
            .iter()
            .filter_map(|r| r.as_str())
            .filter(|name| !out.contains_key(*name))
            .map(|s| format!("--{}", s.replace('_', "-")))
            .collect();
        if !missing.is_empty() {
            return Err(ActError::invalid_params(
                &info.name,
                format!("missing required flag(s): {}", missing.join(", ")),
            ));
        }
    }

    Ok(Value::Object(out))
}

fn normalize_flag(name: &str) -> String {
    name.trim_matches('-').replace('_', "-")
}

/// Resolve a flag (alias or canonical) to a property name. `--no-x` negates
/// booleans.
fn resolve(
    flag: &str,
    props: &Map<String, Value>,
    aliases: &[(String, String)],
    command: &str,
) -> ActResult<(String, bool)> {
    // Direct alias hit.
    for (alias, canonical) in aliases {
        if alias == flag {
            let is_bool = props
                .get(canonical)
                .and_then(|p| p.get("type"))
                .and_then(|t| t.as_str())
                == Some("boolean");
            let negated = is_bool && flag.starts_with("no-");
            return Ok((canonical.clone(), negated));
        }
    }
    // Canonical property name (allow dash form of snake_case names).
    for (prop, _) in props {
        if normalize_flag(prop) == flag {
            return Ok((prop.clone(), false));
        }
    }
    // --no-x negation for boolean canonical names.
    if let Some(stripped) = flag.strip_prefix("no-") {
        for (prop, schema) in props {
            if normalize_flag(prop) == stripped
                && schema.get("type").and_then(|t| t.as_str()) == Some("boolean")
            {
                return Ok((prop.clone(), true));
            }
        }
    }
    Err(ActError::invalid_params(
        command,
        format!("unknown flag --{flag}"),
    ))
}

fn convert(
    command: &str,
    flag: &str,
    canonical: &str,
    prop_type: &str,
    raw: &str,
    schema_prop: &Value,
) -> ActResult<Value> {
    match prop_type {
        "string" => Ok(json!(raw)),
        "integer" => raw.parse::<i64>().map(|n| json!(n)).map_err(|_| {
            ActError::invalid_params(command, format!("--{flag} expects an integer, got '{raw}'"))
        }),
        "number" => raw.parse::<f64>().map(|n| json!(n)).map_err(|_| {
            ActError::invalid_params(command, format!("--{flag} expects a number, got '{raw}'"))
        }),
        "boolean" => match raw {
            "true" => Ok(json!(true)),
            "false" => Ok(json!(false)),
            other => Err(ActError::invalid_params(
                command,
                format!("--{flag} expects true/false, got '{other}'"),
            )),
        },
        "array" => {
            let item_type = schema_prop
                .pointer("/items/type")
                .and_then(|t| t.as_str())
                .unwrap_or("string");
            // Object items are never comma-split (values may contain commas);
            // repeatability of the flag accumulates entries.
            let parts: Vec<&str> = if item_type == "object" {
                vec![raw]
            } else {
                raw.split(',').collect()
            };
            let mut items: Vec<Value> = Vec::new();
            for part in parts {
                match item_type {
                    "integer" => items.push(json!(part.parse::<i64>().map_err(|_| {
                        ActError::invalid_params(
                            command,
                            format!("--{flag} expects integers, got '{part}'"),
                        )
                    })?)),
                    "number" => items.push(json!(part.parse::<f64>().map_err(|_| {
                        ActError::invalid_params(
                            command,
                            format!("--{flag} expects numbers, got '{part}'"),
                        )
                    })?)),
                    "object" => items.push(parse_object_value(command, flag, part)?),
                    _ => items.push(json!(part)),
                }
            }
            Ok(Value::Array(items))
        }
        "object" => parse_object_value(command, flag, raw),
        other => Err(ActError::invalid_params(
            command,
            format!("--{flag}: unsupported parameter type '{other}'"),
        )),
    }
    .map(|v| {
        let _ = canonical;
        v
    })
}

/// `--edit "old=>new"` sugar (array-of-object flags) or raw JSON object.
fn parse_object_value(command: &str, flag: &str, raw: &str) -> ActResult<Value> {
    let raw = raw.trim();
    if raw.starts_with('{') {
        return serde_json::from_str(raw).map_err(|e| {
            ActError::invalid_params(command, format!("--{flag} expects a JSON object: {e}"))
        });
    }
    if flag == "edit" {
        if let Some((old, new)) = raw.split_once("=>") {
            return Ok(json!({ "old": old, "new": new }));
        }
    }
    Err(ActError::invalid_params(
        command,
        format!("--{flag} expects a JSON object or (for --edit) \"old=>new\""),
    ))
}

fn merge(
    out: &mut Map<String, Value>,
    canonical: &str,
    value: Value,
    prop_type: &str,
    command: &str,
    flag: &str,
) -> ActResult<()> {
    if prop_type == "array" {
        let entry = out
            .entry(canonical.to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
        match entry {
            Value::Array(existing) => {
                if let Value::Array(new_items) = value {
                    existing.extend(new_items);
                } else {
                    existing.push(value);
                }
            }
            _ => {
                return Err(ActError::invalid_params(
                    command,
                    format!("--{flag} conflicts with a previous non-array value"),
                ));
            }
        }
    } else if out.contains_key(canonical) {
        return Err(ActError::invalid_params(
            command,
            format!("--{flag} given more than once"),
        ));
    } else {
        out.insert(canonical.to_string(), value);
    }
    Ok(())
}

/// Parse the command's own `example` metadata (used by consistency tests and
/// `act schema` validation so docs can never drift).
pub fn parse_example(def: &CommandDef, info: &CommandInfo) -> ActResult<Value> {
    let example = def
        .example
        .as_deref()
        .ok_or_else(|| ActError::Other(format!("command {} has no example", def.name)))?;
    let args = shell_words(example);
    parse(info, &args)
}

/// Minimal shell-like word splitting (quotes preserved for values).
fn shell_words(input: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    for ch in input.chars() {
        match quote {
            Some(q) => {
                if ch == q {
                    quote = None;
                } else {
                    current.push(ch);
                }
            }
            None => match ch {
                '"' | '\'' => quote = Some(ch),
                c if c.is_whitespace() => {
                    if !current.is_empty() {
                        words.push(std::mem::take(&mut current));
                    }
                }
                c => current.push(c),
            },
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> CommandInfo {
        let def = crate::test_support::fs_read_def();
        CommandInfo {
            name: def.name.as_str().to_string(),
            description: def.description.clone(),
            capability: "read",
            input_schema: def.param_schema.clone(),
            output_schema: def.output_schema.clone(),
            cli_aliases: def.cli_aliases.clone(),
            outputs: Vec::new(),
            cmd_aliases: def.cmd_aliases.clone(),
            example: def.example.clone(),
            path_fields: def.path_fields.clone(),
            url_fields: def.url_fields.clone(),
        }
    }

    #[test]
    fn parses_basic_flags() {
        let v = parse(
            &info(),
            &[
                "--path".into(),
                "a.rs".into(),
                "--offset".into(),
                "5".into(),
                "--limit".into(),
                "10".into(),
            ],
        )
        .unwrap();
        assert_eq!(v["path"], json!("a.rs"));
        assert_eq!(v["offset"], json!(5));
        assert_eq!(v["limit"], json!(10));
    }

    #[test]
    fn inline_equals_form() {
        let v = parse(&info(), &["--path=a.rs".into(), "--limit=7".into()]).unwrap();
        assert_eq!(v["path"], json!("a.rs"));
        assert_eq!(v["limit"], json!(7));
    }

    #[test]
    fn missing_required_rejected() {
        let err = parse(&info(), &[]).unwrap_err();
        assert!(err.to_string().contains("--path"), "{err}");
    }

    #[test]
    fn unknown_flag_rejected() {
        let err = parse(
            &info(),
            &["--path".into(), "a".into(), "--bogus".into(), "1".into()],
        )
        .unwrap_err();
        assert!(matches!(err, ActError::InvalidParams { .. }));
    }

    #[test]
    fn bad_integer_rejected() {
        let err = parse(
            &info(),
            &["--path".into(), "a".into(), "--limit".into(), "abc".into()],
        )
        .unwrap_err();
        assert!(err.to_string().contains("integer"));
    }

    #[test]
    fn no_gitignore_negation() {
        let def = crate::test_support::fs_grep_def();
        let info = CommandInfo {
            name: def.name.as_str().to_string(),
            description: String::new(),
            capability: "read",
            input_schema: def.param_schema.clone(),
            output_schema: Value::Null,
            cli_aliases: def.cli_aliases.clone(),
            outputs: Vec::new(),
            cmd_aliases: def.cmd_aliases.clone(),
            example: None,
            path_fields: vec![],
            url_fields: vec![],
        };
        let v = parse(
            &info,
            &[
                "--root".into(),
                "src".into(),
                "--pattern".into(),
                "x".into(),
                "--no-gitignore".into(),
                "--hidden".into(),
            ],
        )
        .unwrap();
        assert_eq!(v["respect_gitignore"], json!(false));
        assert_eq!(v["include_hidden"], json!(true));
    }

    #[test]
    fn edit_sugar_and_replace_all() {
        let def = crate::test_support::fs_edit_def();
        let info = CommandInfo {
            name: def.name.as_str().to_string(),
            description: String::new(),
            capability: "write",
            input_schema: def.param_schema.clone(),
            output_schema: Value::Null,
            cli_aliases: def.cli_aliases.clone(),
            outputs: Vec::new(),
            cmd_aliases: def.cmd_aliases.clone(),
            example: None,
            path_fields: vec![],
            url_fields: vec![],
        };
        let v = parse(
            &info,
            &[
                "--path".into(),
                "a.rs".into(),
                "--edit".into(),
                "old=>new".into(),
                "--edit".into(),
                "x=>y".into(),
                "--replace-all".into(),
            ],
        )
        .unwrap();
        assert_eq!(v["edits"][0]["old"], json!("old"));
        assert_eq!(v["edits"][1]["new"], json!("y"));
        assert_eq!(v["edits"][0]["replace_all"], json!(true));
        assert_eq!(v["edits"][1]["replace_all"], json!(true));
    }

    #[test]
    fn comma_split_and_repeat_arrays() {
        let def = crate::test_support::fs_create_def();
        let info = CommandInfo {
            name: def.name.as_str().to_string(),
            description: String::new(),
            capability: "write",
            input_schema: def.param_schema.clone(),
            output_schema: Value::Null,
            cli_aliases: def.cli_aliases.clone(),
            outputs: Vec::new(),
            cmd_aliases: def.cmd_aliases.clone(),
            example: None,
            path_fields: vec![],
            url_fields: vec![],
        };
        let v = parse(
            &info,
            &[
                "--paths".into(),
                "a.rs,b.rs".into(),
                "--paths".into(),
                "c.rs".into(),
            ],
        )
        .unwrap();
        assert_eq!(v["paths"], json!(["a.rs", "b.rs", "c.rs"]));
    }
}
