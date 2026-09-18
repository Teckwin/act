//! CLI parser layer — SINGLE RESPONSIBILITY: normalization only.
//!
//! Turns `act Fs_ReadFile --path a.rs --limit 10` into
//! `kernel.exec("Fs_ReadFile", {"path":"a.rs","limit":10}, context)`:
//! command/alias resolution, flag → typed JSON conversion, sugar expansion.
//!
//! It deliberately does NOT validate constraints or security:
//! - schema conformance (required/enum/min/max) → kernel `SchemaGuard`
//! - path normalization (relative → absolute) and sandbox escape blocking
//!   (`../../../`) → kernel `PathGuard`, driven by `Param::verify(PathLike)`
//! - URL policy/SSRF → kernel `UrlGuard`, driven by `Param::verify(UrlLike)`

use act_kernel::error::{ActError, ActResult};
use act_kernel::{CommandDef, CommandInfo};
use serde_json::{json, Map, Value};

/// Full CLI entry parse: `argv → (command, params)`.
///
/// - empty / `-h` / `--help` / `-V` / `--version` → dispatches to the
///   registered `Sys_Help` command (nothing hardcoded at the call site)
/// - leading global flags (`--config`, `--root`) are consumed as runtime
///   context via env (`ACT_CONFIG`) / extra roots handled by the caller
/// - first token is a command name or short alias; the rest are flags
pub fn parse_command(
    argv: &[String],
    resolver: &dyn Fn(&str) -> Option<CommandInfo>,
) -> ActResult<(String, Value)> {
    let mut rest = argv;
    // Global context flags may precede the command name.
    loop {
        match rest.first().map(|s| s.as_str()) {
            Some("--config") | Some("--root") if rest.len() >= 2 => {
                // consumed by the caller (context building); skip here
                rest = &rest[2..];
            }
            Some(token) if token.starts_with("--config=") || token.starts_with("--root=") => {
                rest = &rest[1..];
            }
            _ => break,
        }
    }
    let Some(first) = rest.first() else {
        return Ok(("Sys_Help".into(), json!({})));
    };
    match first.as_str() {
        "-h" | "--help" => return Ok(("Sys_Help".into(), json!({}))),
        "-V" | "--version" => return Ok(("Sys_Help".into(), json!({ "version": true }))),
        _ => {}
    }
    let Some(info) = resolver(first) else {
        return Err(ActError::UnknownCommand(first.clone()));
    };
    let canonical = info.name.clone();
    let params = parse_with(&info, &rest[1..], resolver)?;
    Ok((canonical, params))
}

/// Parse flag arguments into a JSON params object for the given command.
/// Uses the live registry for Sys_Verify's pass-through resolution.
pub fn parse(info: &CommandInfo, args: &[String]) -> ActResult<Value> {
    parse_with(info, args, &|token: &str| crate::sys::resolve_info(token))
}

/// Core parser with an injectable command resolver (tests pass a stub).
pub fn parse_with(
    info: &CommandInfo,
    args: &[String],
    resolver: &dyn Fn(&str) -> Option<CommandInfo>,
) -> ActResult<Value> {
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
    // Pass-through state for Sys_Verify: unknown flags are parsed against the
    // TARGET command's schema and collected under `target_params`.
    let passthrough = info
        .input_schema
        .get("x-passthrough")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let mut target_params: Option<Map<String, Value>> = None;
    let mut target_info_cache: Option<Option<CommandInfo>> = None;
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
                    format!("unexpected positional argument '{token}'"),
                ));
            }
            rest
        } else {
            return Err(ActError::invalid_params(
                &info.name,
                format!("unexpected positional argument '{token}'"),
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

        // Resolve alias → canonical property name. For Sys_Verify
        // (x-passthrough), unknown flags are parsed against the TARGET
        // command's schema and collected under `target_params`.
        let (canonical, negated, schema_prop, in_target) =
            match resolve(&flag_name, props, &aliases, &info.name) {
                Ok((canonical, negated)) => {
                    let prop = props
                        .get(&canonical)
                        .cloned()
                        .unwrap_or_else(|| json!("string"));
                    (canonical, negated, prop, false)
                }
                Err(err) => {
                    if !passthrough {
                        return Err(err);
                    }
                    let target = match &target_info_cache {
                        Some(cached) => cached.clone(),
                        None => {
                            let cached = passthrough_target(args, resolver);
                            target_info_cache = Some(cached.clone());
                            cached
                        }
                    };
                    let Some(target) = target else {
                        return Err(err);
                    };
                    let target_props = target
                        .input_schema
                        .get("properties")
                        .and_then(|v| v.as_object())
                        .cloned()
                        .unwrap_or_default();
                    let target_aliases: Vec<(String, String)> = target
                        .cli_aliases
                        .iter()
                        .map(|(a, b)| (normalize_flag(a), b.clone()))
                        .collect();
                    let (canonical, negated) =
                        resolve(&flag_name, &target_props, &target_aliases, &info.name)
                            .map_err(|_| err)?;
                    let prop = target_props
                        .get(&canonical)
                        .cloned()
                        .unwrap_or(json!("string"));
                    (canonical, negated, prop, true)
                }
            };
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
                        &schema_prop,
                    )?,
                    true,
                )
            }
        };

        if in_target {
            let entry = target_params.get_or_insert_with(Map::new);
            merge(entry, &canonical, value, prop_type, &info.name, &flag_name)?;
        } else {
            merge(
                &mut out, &canonical, value, prop_type, &info.name, &flag_name,
            )?;
        }
        let _ = consumed_next;
        i += 1;
    }

    if let Some(target_params) = target_params {
        out.insert("target_params".into(), Value::Object(target_params));
    }

    // NOTE: no command-specific post-processing here — paired params like
    // Fs_EditFile's --old/--new zip inside the handler; the parser is a pure
    // normalization layer.

    // NOTE: required/enum/min/max validation is intentionally NOT done here —
    // the kernel's SchemaGuard enforces the declared contract for every
    // channel (CLI, MCP, direct exec) uniformly.

    Ok(Value::Object(out))
}

fn normalize_flag(name: &str) -> String {
    name.trim_matches('-').replace('_', "-")
}

/// Pre-scan `--target <name>` / `--target=<name>` / `-t <name>` and resolve
/// the target command metadata for pass-through flag parsing (Sys_Verify).
fn passthrough_target(
    args: &[String],
    resolver: &dyn Fn(&str) -> Option<CommandInfo>,
) -> Option<CommandInfo> {
    let mut i = 0;
    while i < args.len() {
        let token = &args[i];
        let value = if let Some(v) = token.strip_prefix("--target=") {
            Some(v.to_string())
        } else if token == "--target" || token == "-t" {
            args.get(i + 1).cloned()
        } else {
            None
        };
        if let Some(value) = value {
            return resolver(&value);
        }
        i += 1;
    }
    None
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
        "integer" => raw
            .parse::<i64>()
            .map(|n| json!(n))
            .map_err(|_| ActError::invalid_params(command, format!("--{flag} expects an integer, got '{raw}'"))),
        "number" => raw
            .parse::<f64>()
            .map(|n| json!(n))
            .map_err(|_| ActError::invalid_params(command, format!("--{flag} expects a number, got '{raw}'"))),
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
            let mut items: Vec<Value> = Vec::new();
            for part in raw.split(',') {
                match item_type {
                    "integer" => items.push(json!(part.parse::<i64>().map_err(|_| {
                        ActError::invalid_params(command, format!("--{flag} expects integers, got '{part}'"))
                    })?)),
                    "number" => items.push(json!(part.parse::<f64>().map_err(|_| {
                        ActError::invalid_params(command, format!("--{flag} expects numbers, got '{part}'"))
                    })?)),
                    _ => items.push(json!(part)),
                }
            }
            Ok(Value::Array(items))
        }
        other => Err(ActError::invalid_params(
            command,
            format!(
                "--{flag}: type '{other}' is not expressible as a CLI flag; object/array-of-object params are programmatic-channel only (MCP / library exec)"
            ),
        )),
    }
    .map(|v| {
        let _ = canonical;
        v
    })
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
    parse_with(info, &args, &|token: &str| crate::sys::resolve_info(token))
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
            outputs: Vec::new(),
            cmd_aliases: def.cmd_aliases.clone(),
            cli_aliases: def.cli_aliases.clone(),
            example: def.example.clone(),
            path_fields: def.path_fields.clone(),
            url_fields: def.url_fields.clone(),
        }
    }

    #[test]
    fn parses_basic_flags() {
        let v = parse_with(
            &info(),
            &[
                "--path".into(),
                "a.rs".into(),
                "--offset".into(),
                "5".into(),
                "--limit".into(),
                "10".into(),
            ],
            &|_| None,
        )
        .unwrap();
        assert_eq!(v["path"], json!("a.rs"));
        assert_eq!(v["offset"], json!(5));
        assert_eq!(v["limit"], json!(10));
    }

    #[test]
    fn inline_equals_form() {
        let v = parse_with(
            &info(),
            &["--path=a.rs".into(), "--limit=7".into()],
            &|_| None,
        )
        .unwrap();
        assert_eq!(v["path"], json!("a.rs"));
        assert_eq!(v["limit"], json!(7));
    }

    #[test]
    fn unknown_flag_rejected() {
        let err = parse_with(
            &info(),
            &["--path".into(), "a".into(), "--bogus".into(), "1".into()],
            &|_| None,
        )
        .unwrap_err();
        assert!(matches!(err, ActError::InvalidParams { .. }));
    }

    #[test]
    fn bad_integer_rejected() {
        let err = parse_with(
            &info(),
            &["--path".into(), "a".into(), "--limit".into(), "abc".into()],
            &|_| None,
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
            outputs: Vec::new(),
            cmd_aliases: def.cmd_aliases.clone(),
            cli_aliases: def.cli_aliases.clone(),
            example: None,
            path_fields: vec![],
            url_fields: vec![],
        };
        let v = parse_with(
            &info,
            &[
                "--root".into(),
                "src".into(),
                "--pattern".into(),
                "x".into(),
                "--no-gitignore".into(),
                "--hidden".into(),
            ],
            &|_| None,
        )
        .unwrap();
        assert_eq!(v["respect_gitignore"], json!(false));
        assert_eq!(v["include_hidden"], json!(true));
    }

    #[test]
    fn edit_paired_old_new_repeatable() {
        let def = crate::test_support::fs_edit_def();
        let info = CommandInfo {
            name: def.name.as_str().to_string(),
            description: String::new(),
            capability: "write",
            input_schema: def.param_schema.clone(),
            output_schema: Value::Null,
            outputs: Vec::new(),
            cmd_aliases: def.cmd_aliases.clone(),
            cli_aliases: def.cli_aliases.clone(),
            example: None,
            path_fields: vec![],
            url_fields: vec![],
        };
        let v = parse_with(
            &info,
            &[
                "--path".into(),
                "a.rs".into(),
                "--old".into(),
                "old_fn".into(),
                "--new".into(),
                "new_fn".into(),
                "--old".into(),
                "TODO".into(),
                "--new".into(),
                "DONE".into(),
                "--all".into(),
            ],
            &|_| None,
        )
        .unwrap();
        assert_eq!(v["old"], json!(["old_fn", "TODO"]));
        assert_eq!(v["new"], json!(["new_fn", "DONE"]));
        assert_eq!(v["replace_all"], json!(true));
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
            outputs: Vec::new(),
            cmd_aliases: def.cmd_aliases.clone(),
            cli_aliases: def.cli_aliases.clone(),
            example: None,
            path_fields: vec![],
            url_fields: vec![],
        };
        let v = parse_with(
            &info,
            &[
                "--paths".into(),
                "a.rs,b.rs".into(),
                "--paths".into(),
                "c.rs".into(),
            ],
            &|_| None,
        )
        .unwrap();
        assert_eq!(v["paths"], json!(["a.rs", "b.rs", "c.rs"]));
    }

    #[test]
    fn parse_command_dispatches_help_and_aliases() {
        let resolver = |token: &str| -> Option<CommandInfo> {
            match token {
                "fr" | "Fs_ReadFile" => Some(info()),
                _ => None,
            }
        };
        // help forwarding
        let (cmd, _) = parse_command(&[], &resolver).unwrap();
        assert_eq!(cmd, "Sys_Help");
        let (cmd, params) = parse_command(&["--help".into()], &resolver).unwrap();
        assert_eq!(cmd, "Sys_Help");
        assert!(params.get("version").is_none());
        let (cmd, params) = parse_command(&["-V".into()], &resolver).unwrap();
        assert_eq!(cmd, "Sys_Help");
        assert_eq!(params["version"], json!(true));
        // alias + flags
        let (cmd, params) =
            parse_command(&["fr".into(), "-p".into(), "a.rs".into()], &resolver).unwrap();
        assert_eq!(cmd, "Fs_ReadFile");
        assert_eq!(params["path"], json!("a.rs"));
        // unknown
        assert!(matches!(
            parse_command(&["Nope".into()], &resolver).unwrap_err(),
            ActError::UnknownCommand(_)
        ));
    }
}
