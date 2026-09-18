//! LimitGuard: caps well-known numeric parameters before execution.

use serde_json::Value;

use crate::config::Limits;
use crate::error::{ActError, ActResult};

/// Validate numeric request parameters against configured caps.
/// Unknown numeric parameters are left to per-command validation.
pub fn check_numeric(params: &Value, limits: &Limits) -> ActResult<()> {
    let get_u64 = |key: &str| -> Option<u64> { params.get(key).and_then(|v| v.as_u64()) };

    if let Some(n) = get_u64("max_bytes") {
        if n == 0 || n > limits.fetch_max_bytes {
            return Err(ActError::LimitExceeded {
                reason: format!("max_bytes={n} exceeds cap {}", limits.fetch_max_bytes),
            });
        }
    }
    if let Some(n) = get_u64("max_results") {
        if n == 0 || n > limits.max_find_results.max(limits.max_grep_results) as u64 {
            return Err(ActError::LimitExceeded {
                reason: format!("max_results={n} exceeds cap {}", limits.max_find_results),
            });
        }
    }
    if let Some(n) = get_u64("limit") {
        if n > 200_000 {
            return Err(ActError::LimitExceeded {
                reason: format!("limit={n} exceeds 200000"),
            });
        }
    }
    if let Some(n) = get_u64("offset") {
        if n > 100_000_000 {
            return Err(ActError::LimitExceeded {
                reason: format!("offset={n} is out of range"),
            });
        }
    }
    if let Some(n) = get_u64("depth") {
        if n > limits.max_depth as u64 {
            return Err(ActError::LimitExceeded {
                reason: format!("depth={n} exceeds cap {}", limits.max_depth),
            });
        }
    }
    if let Some(n) = get_u64("timeout_ms") {
        if n == 0 || n > limits.web_timeout_ms * 10 {
            return Err(ActError::LimitExceeded {
                reason: format!("timeout_ms={n} exceeds cap {}", limits.web_timeout_ms * 10),
            });
        }
    }
    if let Some(n) = get_u64("context_lines") {
        if n > 10 {
            return Err(ActError::LimitExceeded {
                reason: format!("context_lines={n} exceeds 10"),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn oversize_max_bytes_rejected() {
        let limits = Limits::default();
        let err = check_numeric(&json!({"max_bytes": 999_999_999}), &limits).unwrap_err();
        assert!(matches!(err, ActError::LimitExceeded { .. }));
    }

    #[test]
    fn within_caps_ok() {
        let limits = Limits::default();
        check_numeric(
            &json!({"max_bytes": 1024, "max_results": 50, "depth": 8, "limit": 2000}),
            &limits,
        )
        .expect("ok");
    }

    #[test]
    fn bad_depth_rejected() {
        let err = check_numeric(&json!({"depth": 999}), &Limits::default()).unwrap_err();
        assert!(matches!(err, ActError::LimitExceeded { .. }));
    }
}
