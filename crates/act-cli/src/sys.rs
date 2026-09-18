//! Sys_* meta commands: every built-in management verb (list / verify /
//! schema / package / install / serve) is registered through the same
//! CommandBuilder contract pipeline as Fs_*/Web_* — one registry, one flag
//! parser, one doc generator. No orphan CLI subcommands outside the kernel.
//!
//! Sys handlers need to read the live command registry (e.g. Sys_List),
//! which lives inside the very manager they are registered into. The peer
//! manager is therefore late-bound: `main` registers everything first, then
//! calls `bind_manager`.

use std::sync::{Arc, Mutex, OnceLock};

use act_kernel::builder::{CommandBuilder, Param};
use act_kernel::error::{ActError, ActResult};
use act_kernel::{
    Capability, CommandDef, CommandHandler, CommandManager, InvokeMode, SandboxContext,
};
use async_trait::async_trait;
use serde_json::{json, Value};

static SYS_MANAGER: OnceLock<Mutex<Option<Arc<CommandManager>>>> = OnceLock::new();

/// Late-bind the live manager (call after all commands are registered).
pub fn bind_manager(manager: Arc<CommandManager>) {
    let cell = SYS_MANAGER.get_or_init(|| Mutex::new(None));
    *cell.lock().unwrap() = Some(manager);
}

pub fn manager() -> ActResult<Arc<CommandManager>> {
    SYS_MANAGER
        .get()
        .and_then(|cell| cell.lock().unwrap().clone())
        .ok_or_else(|| ActError::Other("sys manager not bound yet".into()))
}

/// Resolve a command name or alias to its metadata (used by the flag parser
/// for Sys_Verify's pass-through flags).
pub fn resolve_info(token: &str) -> Option<act_kernel::CommandInfo> {
    SYS_MANAGER
        .get()
        .and_then(|cell| cell.lock().unwrap().clone())
        .and_then(|m| m.resolve_def(token))
        .map(|def| act_kernel::CommandInfo {
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
}

pub fn register_all(manager: &CommandManager) -> ActResult<()> {
    // Builtin domain "Sys" must be whitelisted before first Sys_ command.
    manager.register_domain("Sys")?;
    for mut def in definitions() {
        if def.name.as_str() == "Sys_Verify" {
            // Mark pass-through so the flag parser resolves target flags.
            def.param_schema["x-passthrough"] = json!(true);
        }
        manager.register(def)?;
    }
    Ok(())
}

fn definitions() -> Vec<CommandDef> {
    vec![
        sys_list(),
        sys_verify(),
        sys_schema(),
        sys_package(),
        sys_install(),
        sys_serve(),
    ]
    .into_iter()
    .collect::<Result<Vec<_>, _>>()
    .expect("builtin Sys definitions must be valid")
}

// ---------- Sys_List ----------

struct SysList;

#[async_trait]
impl CommandHandler for SysList {
    async fn execute(&self, params: Value, _ctx: &SandboxContext) -> ActResult<Value> {
        let json_mode = params
            .get("json")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let manager = manager()?;
        if json_mode {
            return Ok(json!({
                "ok": true, "command": "Sys_List", "count": manager.list().len(),
                "commands": manager.list(),
            }));
        }
        let mut table = String::from("COMMAND           ALIAS  CAP   DESCRIPTION\n");
        for info in manager.list() {
            let alias = info.cmd_aliases.first().map(String::as_str).unwrap_or("-");
            let desc: String = info.description.chars().take(72).collect();
            table.push_str(&format!(
                "{:<17} {:<6} {:<5} {}\n",
                info.name, alias, info.capability, desc
            ));
        }
        Ok(json!({
            "ok": true, "command": "Sys_List",
            "count": manager.list().len(),
            "table": table,
        }))
    }
}

fn sys_list() -> ActResult<CommandDef> {
    CommandBuilder::new(
        "Sys_List",
        "List every registered command (name, alias, capability, description).",
        Capability::Meta,
        "list",
    )
    .param(
        Param::boolean("json")
            .default(json!(false))
            .desc("输出结构化 JSON（含全量契约元数据）"),
    )
    .output_done(json!({
        "type": "object", "required": ["ok", "command", "count"],
        "properties": {
            "ok": { "type": "boolean" }, "command": { "const": "Sys_List" },
            "count": { "type": "integer" },
            "table": { "type": "string", "description": "人读表格（json=false 时）" },
            "commands": { "type": "array", "description": "json=true 时的全量命令元数据" }
        }
    }))
    .example("--json")
    .bind(Arc::new(SysList))
}

// ---------- Sys_Verify ----------

struct SysVerify;

#[async_trait]
impl CommandHandler for SysVerify {
    async fn execute(&self, params: Value, _ctx: &SandboxContext) -> ActResult<Value> {
        let manager = manager()?;
        let target = params
            .get("target")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ActError::invalid_params("Sys_Verify", "missing --target"))?;
        let def = manager
            .resolve_def(target)
            .ok_or_else(|| ActError::UnknownCommand(target.to_string()))?;
        let canonical = def.name.as_str().to_string();
        let target_params = params
            .get("target_params")
            .cloned()
            .unwrap_or_else(|| json!({}));
        match manager
            .verify_only(&canonical, &target_params, InvokeMode::Cli)
            .await
        {
            Ok(verification) => Ok(json!({
                "ok": true, "command": "Sys_Verify", "allowed": true, "target": canonical,
                "paths": verification.paths.iter().map(|(raw, abs, _)| json!({
                    "input": raw, "resolved": abs.to_string_lossy().replace('\\', "/"),
                })).collect::<Vec<_>>(),
                "urls": verification.urls.iter().map(|(raw, url)| json!({
                    "input": raw, "resolved": url.to_string(),
                })).collect::<Vec<_>>(),
            })),
            Err(err) => Ok(json!({
                "ok": true, "command": "Sys_Verify", "allowed": false, "target": canonical,
                "error": err.to_string(), "code": err.code(),
            })),
        }
    }
}

fn sys_verify() -> ActResult<CommandDef> {
    CommandBuilder::new(
        "Sys_Verify",
        "Permission dry-run for a target command: resolves paths/urls through the guard pipeline WITHOUT executing.",
        Capability::Meta,
        "verify",
    )
    .param(
        Param::string("target")
            .alias("t")
            .required()
            .desc("目标命令名或别名（如 Fs_ReadFile / fr）"),
    )
    .output_done(json!({
        "type": "object", "required": ["ok", "command", "allowed", "target"],
        "properties": {
            "ok": { "type": "boolean" }, "command": { "const": "Sys_Verify" },
            "allowed": { "type": "boolean" }, "target": { "type": "string" },
            "paths": { "type": "array", "description": "放行时：input→resolved 路径映射" },
            "urls": { "type": "array", "description": "放行时：input→resolved URL 映射" },
            "error": { "type": "string", "description": "拒绝时：错误说明" },
            "code": { "type": "string", "description": "拒绝时：错误码" }
        }
    }))
    .output_fail("permission_denied", json!({
        "type": "object", "description": "目标命令参数越界时：allowed=false + code/message",
        "properties": { "code": { "const": "permission_denied" }, "message": { "type": "string" } }
    }))
    .example("--target Fs_ReadFile --path src/main.rs")
    .bind(Arc::new(SysVerify))
}

/// Sys_Verify enables flag pass-through — see flags::parse_with.)

// (schema flag: x-passthrough injected below)

// ---------- Sys_Schema ----------

struct SysSchema;

#[async_trait]
impl CommandHandler for SysSchema {
    async fn execute(&self, _params: Value, _ctx: &SandboxContext) -> ActResult<Value> {
        let manager = manager()?;
        Ok(crate::schema::build(&manager))
    }
}

fn sys_schema() -> ActResult<CommandDef> {
    CommandBuilder::new(
        "Sys_Schema",
        "Print the full machine-readable contract: every command schema + aliases + output variants + error table + effective limits.",
        Capability::Meta,
        "schema",
    )
    .output_done(json!({
        "type": "object",
        "required": ["name", "version", "commands", "errors", "limits"],
        "properties": {
            "name": { "const": "agent-core-tools" }, "version": { "type": "string" },
            "commands": { "type": "array" }, "errors": { "type": "array" }, "limits": { "type": "object" },
            "envelopes": { "type": "object" }
        }
    }))
    .example("")
    .bind(Arc::new(SysSchema))
}

// ---------- Sys_Package ----------

struct SysPackage;

#[async_trait]
impl CommandHandler for SysPackage {
    async fn execute(&self, params: Value, _ctx: &SandboxContext) -> ActResult<Value> {
        let out = params
            .get("out")
            .and_then(|v| v.as_str())
            .unwrap_or("dist")
            .to_string();
        let zip = crate::package::run(std::path::Path::new(&out))?;
        Ok(json!({
            "ok": true, "command": "Sys_Package",
            "staged": format!("{out}/agent-core-tools"),
            "zip": zip.to_string_lossy().replace('\\', "/"),
        }))
    }
}

fn sys_package() -> ActResult<CommandDef> {
    CommandBuilder::new(
        "Sys_Package",
        "Stage the self-contained skill bundle (SKILL.md + schema.json + mcp.json, all compiled from this binary) and zip it.",
        Capability::Meta,
        "package",
    )
    .param(Param::string("out").alias("o").default(json!("dist")).desc("输出目录"))
    .output_done(json!({
        "type": "object", "required": ["ok", "command", "staged", "zip"],
        "properties": {
            "ok": { "type": "boolean" }, "command": { "const": "Sys_Package" },
            "staged": { "type": "string" }, "zip": { "type": "string" }
        }
    }))
    .example("--out dist")
    .bind(Arc::new(SysPackage))
}

// ---------- Sys_Install ----------

struct SysInstall;

#[async_trait]
impl CommandHandler for SysInstall {
    async fn execute(&self, params: Value, _ctx: &SandboxContext) -> ActResult<Value> {
        let project = params
            .get("project")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let user = params
            .get("user")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let exe = params.get("exe").and_then(|v| v.as_str()).map(String::from);
        let with_mcp = params
            .get("with_mcp")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let force = params
            .get("force")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        crate::install::run(project, user, exe, with_mcp, force)?;
        Ok(json!({
            "ok": true, "command": "Sys_Install",
            "project": project, "user": user, "with_mcp": with_mcp,
        }))
    }
}

fn sys_install() -> ActResult<CommandDef> {
    CommandBuilder::new(
        "Sys_Install",
        "Install the skill files (generated SKILL.md/schema.json/mcp.json) into --project (.claude/skills/) and/or --user (~/.claude/skills/). CLI-first: no MCP registration unless --with-mcp.",
        Capability::Meta,
        "install",
    )
    .param(Param::boolean("project").desc("安装到当前项目 .claude/skills/"))
    .param(Param::boolean("user").desc("安装到用户目录 ~/.claude/skills/"))
    .param(Param::string("exe").desc("MCP 注册用的可执行路径（默认本二进制的真实路径）"))
    .param(Param::boolean("with_mcp").desc("同时写入 .mcp.json（项目级，注册本二进制为 MCP server）"))
    .param(Param::boolean("force").desc("覆盖已存在的 .mcp.json 条目"))
    .output_done(json!({
        "type": "object", "required": ["ok", "command"],
        "properties": {
            "ok": { "type": "boolean" }, "command": { "const": "Sys_Install" },
            "project": { "type": "boolean" }, "user": { "type": "boolean" }, "with_mcp": { "type": "boolean" }
        }
    }))
    .output_fail("invalid_params", json!({
        "type": "object", "description": "未选择 --project/--user 时",
        "properties": { "code": { "const": "invalid_params" }, "message": { "type": "string" } }
    }))
    .example("--user")
    .bind(Arc::new(SysInstall))
}

// ---------- Sys_Serve ----------

struct SysServe;

#[async_trait]
impl CommandHandler for SysServe {
    async fn execute(&self, _params: Value, _ctx: &SandboxContext) -> ActResult<Value> {
        // Long-running: serves MCP stdio until EOF, then returns normally.
        let manager = manager()?;
        crate::mcp::serve_arc(manager.clone()).await?;
        Ok(json!({ "ok": true, "command": "Sys_Serve" }))
    }
}

fn sys_serve() -> ActResult<CommandDef> {
    let mut def = CommandBuilder::new(
        "Sys_Serve",
        "Run as an MCP stdio server (JSON-RPC 2.0: initialize/tools list/tools call/ping). Optional integration channel; the CLI is the primary interface.",
        Capability::Meta,
        "serve",
    )
    .output_done(json!({
        "type": "object", "required": ["ok", "command"],
        "properties": { "ok": { "type": "boolean" }, "command": { "const": "Sys_Serve" } }
    }))
    .example("")
    .bind(Arc::new(SysServe))?;
    def.cmd_aliases.push("mcp".into());
    Ok(def)
}
