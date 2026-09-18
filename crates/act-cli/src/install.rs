//! `act install`: write .mcp.json + skill files for project or user scope.

use act_kernel::error::{ActError, ActResult};

const SKILL_MD: &str = include_str!("../../../skill/SKILL.md");
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
        let skill_dir = cwd.join(".claude/skills").join(SKILL_NAME);
        std::fs::create_dir_all(&skill_dir).map_err(ActError::Io)?;
        std::fs::write(skill_dir.join("SKILL.md"), SKILL_MD).map_err(ActError::Io)?;
        println!(
            "installed: {} (mcp server 'act')",
            cwd.join(".mcp.json").display()
        );
        println!("installed: {}", skill_dir.join("SKILL.md").display());
    }
    if user {
        let home = user_home()?;
        let skill_dir = home.join(".claude/skills").join(SKILL_NAME);
        std::fs::create_dir_all(&skill_dir).map_err(ActError::Io)?;
        std::fs::write(skill_dir.join("SKILL.md"), SKILL_MD).map_err(ActError::Io)?;
        println!("installed: {}", skill_dir.join("SKILL.md").display());
        println!("to enable MCP at user level add to your MCP config:");
        println!(
            "  {{\"mcpServers\": {{\"act\": {{\"command\": \"{exe}\", \"args\": [\"mcp\"]}}}}}}"
        );
    }
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
    if !root.get("mcpServers").is_some() {
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
    let bytes = serde_json::to_vec_pretty(&root)
        .map_err(|e| ActError::Other(format!("serialize .mcp.json: {e}")))?;
    std::fs::write(&path, bytes).map_err(ActError::Io)?;
    Ok(())
}

fn user_home() -> ActResult<std::path::PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(std::path::PathBuf::from)
        .ok_or_else(|| ActError::Other("cannot determine user home directory".into()))
}
