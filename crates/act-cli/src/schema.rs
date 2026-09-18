//! Machine-readable command contract: all command schemas, the error code
//! table and effective limits, emitted as `schema.json` (also used by the
//! `act schema` CLI command and the skill packaging/install flows).

use act_kernel::error::ActResult;
use act_kernel::{ActConfig, CommandManager};
use serde_json::{json, Value};

/// Build the full contract JSON from a live manager.
pub fn build(manager: &CommandManager) -> Value {
    let config = manager.config();
    json!({
        "name": "agent-core-tools",
        "version": env!("CARGO_PKG_VERSION"),
        "commands": manager.list().iter().map(|c| json!({
            "name": c.name,
            "description": c.description,
            "capability": c.capability,
            "inputSchema": c.input_schema,
            "outputSchema": c.output_schema,
            "cmdAliases": c.cmd_aliases,
            "cliAliases": c.cli_aliases,
            "outputs": c.outputs,
            "example": c.example,
            "pathFields": c.path_fields,
            "urlFields": c.url_fields,
        })).collect::<Vec<_>>(),
        "errors": error_table(),
        "limits": {
            "max_batch": config.limits.max_batch,
            "max_read_bytes": config.limits.max_read_bytes,
            "max_write_bytes": config.limits.max_write_bytes,
            "fetch_max_bytes": config.limits.fetch_max_bytes,
            "max_find_results": config.limits.max_find_results,
            "max_grep_results": config.limits.max_grep_results,
            "max_depth": config.limits.max_depth,
            "command_timeout_secs": config.limits.command_timeout_secs,
            "web_timeout_ms": config.limits.web_timeout_ms,
            "max_redirects": config.url.max_redirects,
            "max_inline_bytes": config.output.max_inline_bytes,
            "max_summary_chars": config.output.compression.max_summary_chars
        },
        "envelopes": {
            "flat": crate::gen::envelope_summary(manager)["flat"],
            "batch": crate::gen::envelope_summary(manager)["batch"],
            "overflow": {
                "when": "serialized result > limits.max_inline_bytes",
                "shape": { "truncated": "true", "compressed": "bool", "command": "string", "original_bytes": "int", "summary": "string", "full_content_path": "string" }
            }
        }
    })
}

/// Build the contract with a detached default manager (no project config
/// lookup) — used by packaging and skill installation so the generated
/// schema.json never depends on the build machine's local configuration.
pub fn build_default() -> ActResult<Value> {
    let manager = detached_manager()?;
    Ok(build(&manager))
}

/// A manager built from default config (no project lookup) — shared by
/// packaging, skill installation and doc generation so generated artifacts
/// never depend on the build machine's local configuration. Also late-binds
/// the Sys peer so pass-through/example validation works standalone.
pub fn detached_manager() -> ActResult<std::sync::Arc<CommandManager>> {
    let cwd = std::env::current_dir().map_err(act_kernel::ActError::Io)?;
    let config = ActConfig {
        roots: vec![cwd],
        ..Default::default()
    };
    let manager = std::sync::Arc::new(CommandManager::new(config)?);
    act_fs::register_all(&manager)?;
    act_web::register_all(&manager)?;
    crate::sys::register_all(&manager)?;
    crate::sys::bind_manager(manager.clone());
    Ok(manager)
}

pub fn error_table() -> Value {
    json!([
        { "code": "permission_denied", "meaning": "路径出根/受保护/URL 策略拒绝", "exitCode": 2, "mcp": "isError:true", "granularity": "whole-call", "guards": ["PathGuard", "ProtectGuard", "UrlGuard", "SandboxRoot"] },
        { "code": "limit_exceeded", "meaning": "超出限额（批量/字节/深度/重定向）", "exitCode": 2, "mcp": "isError:true", "granularity": "whole-call", "guards": ["LimitGuard"] },
        { "code": "capability_disabled", "meaning": "read/write/net 能力被禁用于当前模式", "exitCode": 2, "mcp": "isError:true", "granularity": "whole-call", "guards": ["CapabilityGuard"] },
        { "code": "unknown_command", "meaning": "命令名未注册", "exitCode": 3, "mcp": "isError:true", "granularity": "whole-call", "guards": [] },
        { "code": "invalid_params", "meaning": "参数缺失/类型错/语义非法", "exitCode": 4, "mcp": "isError:true", "granularity": "whole-call", "guards": [] },
        { "code": "execution_failed", "meaning": "运行期失败（文件不存在/HTTP 4xx 5xx/编码不可表示等）", "exitCode": 5, "mcp": "per-item ok:false", "granularity": "per-item", "guards": [] },
        { "code": "io_error", "meaning": "系统 IO 错误", "exitCode": 5, "mcp": "per-item ok:false", "granularity": "per-item", "guards": [] },
        { "code": "other", "meaning": "内部错误", "exitCode": 5, "mcp": "isError:true 或 per-item", "granularity": "whole-call", "guards": [] },
        { "code": "timeout", "meaning": "命令总超时", "exitCode": 6, "mcp": "isError:true", "granularity": "whole-call", "guards": [] },
        { "code": "config_error", "meaning": "配置文件/沙箱根错误", "exitCode": 7, "mcp": "isError:true", "granularity": "whole-call", "guards": [] }
    ])
}
