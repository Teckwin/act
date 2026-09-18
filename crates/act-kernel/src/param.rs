//! Parameter field extraction.
//!
//! Every command declares which parameter positions hold filesystem paths and
//! which hold URLs, as JSON-Pointer-ish patterns with `*` wildcards, e.g.
//! `/paths/*`, `/files/*/path`, `/moves/*/from`, `/urls/*`, `/root`.
//! The permission verifier extracts these values for validation before a
//! handler ever runs, and the audit log records them verbatim.

use serde_json::Value;

/// Extract all strings matched by a pointer pattern such as
/// `/paths/*`, `/files/*/path` or `/root`.
pub fn extract_strings(params: &Value, pointer: &str) -> Vec<String> {
    let mut out = Vec::new();
    let segs: Vec<&str> = pointer
        .trim_start_matches('/')
        .split('/')
        .filter(|s| !s.is_empty())
        .collect();
    walk(params, &segs, &mut out);
    out
}

fn walk(value: &Value, segs: &[&str], out: &mut Vec<String>) {
    match segs.split_first() {
        None => match value {
            Value::String(s) => out.push(s.clone()),
            Value::Array(items) => {
                for item in items {
                    if let Value::String(s) = item {
                        out.push(s.clone());
                    }
                }
            }
            _ => {}
        },
        Some((seg, rest)) => match (*seg, value) {
            ("*", Value::Array(items)) => {
                for item in items {
                    walk(item, rest, out);
                }
            }
            ("*", Value::Object(map)) => {
                for item in map.values() {
                    walk(item, rest, out);
                }
            }
            (key, Value::Object(map)) => {
                if let Some(next) = map.get(key) {
                    walk(next, rest, out);
                }
            }
            _ => {}
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_flat_array() {
        let params = json!({"paths": ["a.txt", "b.txt"]});
        assert_eq!(extract_strings(&params, "/paths/*"), vec!["a.txt", "b.txt"]);
    }

    #[test]
    fn extracts_array_without_wildcard() {
        let params = json!({"paths": ["a.txt"]});
        assert_eq!(extract_strings(&params, "/paths"), vec!["a.txt"]);
    }

    #[test]
    fn extracts_nested_fields() {
        let params = json!({"moves": [{"from": "a", "to": "b"}, {"from": "c", "to": "d"}], "files": [{"path": "x", "content": "hi"}]});
        assert_eq!(extract_strings(&params, "/moves/*/from"), vec!["a", "c"]);
        assert_eq!(extract_strings(&params, "/moves/*/to"), vec!["b", "d"]);
        assert_eq!(extract_strings(&params, "/files/*/path"), vec!["x"]);
    }

    #[test]
    fn extracts_scalar() {
        let params = json!({"root": "src"});
        assert_eq!(extract_strings(&params, "/root"), vec!["src"]);
    }

    #[test]
    fn missing_field_yields_empty() {
        let params = json!({});
        assert!(extract_strings(&params, "/paths/*").is_empty());
    }
}
