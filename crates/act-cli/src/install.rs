//! `act install`: write .mcp.json + skill files for project or user scope.
//!
//! The bundled skill consists of SKILL.md + schema.json (generated) +
//! mcp.json (template). When run from an extracted distribution zip the
//! binary sits inside the skill directory, so `current_exe()` yields the
//! user's real path — no build-machine paths ever leak into configs.

use act_kernel::error::{ActError, ActResult};
use std::path::PathBuf;

const SKILL_MD: &str = include_str!("../../../packaging/skill/agent-core-tools/SKILL.md");
const MCP_JSON_TEMPLATE: &str = include_str!("../../../packaging/skill/agent-core-tools/mcp.json");
const SKILL_NAME: &str = "agent-core-tools";

pub fn run(project: bool, user: bool, exe_override: Option<String>, force: bool) -> ActResult<()> {
    if !project && !user {
        return Err(ActError::invalid_params(
            "install",
            "choose --project and/or --user",
        ));
    }
    let exe = match exe_override {
        Some(path) => path,
        None => std::env::current_exe()
            .map_err(ActError::Io)?
            .to_string_lossy()
            .replace('\\', "/"),
    };

    if project {
        let cwd = std::env::current_dir().map_err(ActError::Io)?;
        write_mcp_json(&cwd, &exe, force)?;
        write_skill_dir(&cwd.join(".claude/skills"))?;
        println!(
            "installed: {} (mcp server 'act')",
            cwd.join(".mcp.json").display()
        );
    }
    if user {
        let home = user_home()?;
        write_skill_dir(&home.join(".claude/skills"))?;
        println!(
            "installed: {}",
            home.join(".claude/skills").join(SKILL_NAME).display()
        );
        println!("to enable MCP at user level add to your MCP config:");
        println!(
            "  {{\"mcpServers\": {{\"act\": {{\"command\": \"{exe}\", \"args\": [\"mcp\"]}}}}}}"
        );
    }
    Ok(())
}

/// Write SKILL.md + schema.json + mcp.json into `<base>/<skill-name>/`.
fn write_skill_dir(base: &std::path::Path) -> ActResult<()> {
    let skill_dir = base.join(SKILL_NAME);
    std::fs::create_dir_all(&skill_dir).map_err(ActError::Io)?;
    std::fs::write(skill_dir.join("SKILL.md"), SKILL_MD).map_err(ActError::Io)?;
    let schema = crate::schema::build_default()?;
    let pretty = serde_json::to_vec_pretty(&schema)
        .map_err(|e| ActError::Other(format!("serialize schema: {e}")))?;
    std::fs::write(skill_dir.join("schema.json"), pretty).map_err(ActError::Io)?;
    std::fs::write(skill_dir.join("mcp.json"), MCP_JSON_TEMPLATE).map_err(ActError::Io)?;
    Ok(())
}

fn write_mcp_json(dir: &std::path::Path, exe: &str, force: bool) -> ActResult<()> {
    let path = dir.join(".mcp.json");
    let entry = serde_json::json!({
        "command": exe,
        "args": ["mcp"],
        "env": {}
    });
    let mut root: serde_json::Value = if path.is_file() {
        serde_json::from_str(&std::fs::read_to_string(&path).map_err(ActError::Io)?)
            .unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };
    if root.get("mcpServers").is_none() {
        root["mcpServers"] = serde_json::json!({});
    }
    let exists = root
        .pointer("/mcpServers/act")
        .map(|v| !v.is_null())
        .unwrap_or(false);
    if exists && !force {
        return Err(ActError::Other(
            ".mcp.json already has an 'act' server; use --force to overwrite".into(),
        ));
    }
    root["mcpServers"]["act"] = entry;
    std::fs::write(
        &path,
        serde_json::to_vec_pretty(&root)
            .map_err(|e| ActError::Other(format!("serialize .mcp.json: {e}")))?,
    )
    .map_err(ActError::Io)?;
    Ok(())
}

fn user_home() -> ActResult<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(std::path::PathBuf::from)
        .ok_or_else(|| ActError::Other("cannot determine user home directory".into()))
}
