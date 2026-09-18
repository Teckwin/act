//! End-to-end tests: unified dynamic CLI (flags mode, aliases, Sys_* meta
//! commands), generated docs consistency, MCP stdio protocol, packaging.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};

const ACT: &str = env!("CARGO_BIN_EXE_act");

struct ChildIo {
    child: Child,
}

impl ChildIo {
    fn spawn(args: &[&str], cwd: &std::path::Path) -> Self {
        let child = Command::new(ACT)
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn act");
        Self { child }
    }

    fn send_line(&mut self, line: &str) {
        let stdin = self.child.stdin.as_mut().expect("stdin");
        stdin.write_all(line.as_bytes()).unwrap();
        stdin.write_all(b"\n").unwrap();
        stdin.flush().unwrap();
    }

    fn read_line(&mut self) -> String {
        let stdout = self.child.stdout.as_mut().expect("stdout");
        let mut reader = BufReader::new(stdout);
        let mut line = String::new();
        reader.read_line(&mut line).expect("read line");
        line
    }
}

impl Drop for ChildIo {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ---------- Unified dynamic CLI ----------

#[test]
fn flags_mode_read_utf8() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("hello.txt"), "hello 世界").unwrap();
    let output = Command::new(ACT)
        .args(["Fs_ReadFile", "--path", "hello.txt"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["content"], serde_json::json!("hello 世界"));
    assert_eq!(value["encoding"], serde_json::json!("utf-8"));
    assert!(value.get("results").is_none(), "flat envelope expected");
}

#[test]
fn flags_mode_write_gbk_read_edit_flow() {
    let tmp = tempfile::tempdir().unwrap();
    let out = Command::new(ACT)
        .args([
            "Fs_WriteFile",
            "--path",
            "cn.txt",
            "--content",
            "旧的内容",
            "--encoding",
            "gbk",
        ])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let out = Command::new(ACT)
        .args(["Fs_EditFile", "--path", "cn.txt", "--edit", "旧=>新"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(out.status.success());
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["replacements"], serde_json::json!(1));
    assert_eq!(value["encoding"], serde_json::json!("gbk"));
    let out = Command::new(ACT)
        .args(["Fs_ReadFile", "--path", "cn.txt"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["content"], serde_json::json!("新的内容"));
}

#[test]
fn flags_mode_batch_create_with_commas() {
    let tmp = tempfile::tempdir().unwrap();
    let out = Command::new(ACT)
        .args(["Fs_CreateFile", "--paths", "a.rs,b.rs,docs/c.md"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(out.status.success());
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["summary"]["succeeded"], serde_json::json!(3));
    assert!(tmp.path().join("docs/c.md").is_file());
}

#[test]
fn flags_mode_missing_required_flag_exit_4() {
    let tmp = tempfile::tempdir().unwrap();
    let out = Command::new(ACT)
        .args(["Fs_ReadFile"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&out.stderr).contains("--path"));
}

#[test]
fn flags_mode_unknown_flag_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("a.txt"), b"x").unwrap();
    let out = Command::new(ACT)
        .args(["Fs_ReadFile", "--path", "a.txt", "--bogus", "1"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(4));
    assert!(String::from_utf8_lossy(&out.stderr).contains("unknown flag"));
}

#[test]
fn flags_mode_traversal_denied_exit_2() {
    let tmp = tempfile::tempdir().unwrap();
    let out = Command::new(ACT)
        .args(["Fs_ReadFile", "--path", "../../x"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn alias_short_command_and_short_flags() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("alias.txt"), "alias ok 中文").unwrap();
    let out = Command::new(ACT)
        .args(["fr", "-p", "alias.txt"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["command"], serde_json::json!("Fs_ReadFile"));
    assert_eq!(value["content"], serde_json::json!("alias ok 中文"));

    let out = Command::new(ACT)
        .args(["fw", "-p", "new.txt", "-c", "短参写入"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(out.status.success());
    let out = Command::new(ACT)
        .args(["fr", "-p", "new.txt"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["content"], serde_json::json!("短参写入"));
}

// ---------- Sys_* meta commands (unified registration) ----------

#[test]
fn sys_list_shows_all_commands_with_aliases() {
    let tmp = tempfile::tempdir().unwrap();
    let out = Command::new(ACT)
        .args(["Sys_List", "--json"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(out.status.success());
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let commands = value["commands"].as_array().unwrap();
    assert!(
        commands.len() >= 25,
        "expected >= 25 commands, got {}",
        commands.len()
    );
    let names: Vec<&str> = commands
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    for expected in [
        "Fs_ReadFile",
        "Web_Research",
        "Sys_List",
        "Sys_Verify",
        "Sys_Schema",
        "Sys_Package",
        "Sys_Install",
        "Sys_Serve",
    ] {
        assert!(names.contains(&expected), "missing {expected}");
    }
    // alias smoke via `act list`
    let out = Command::new(ACT)
        .args(["list"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("COMMAND") && text.contains("Fs_ReadFile"));
}

#[test]
fn sys_verify_passthrough_dry_run() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("a.txt"), b"x").unwrap();
    let out = Command::new(ACT)
        .args(["Sys_Verify", "--target", "Fs_ReadFile", "--path", "a.txt"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(out.status.success());
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["allowed"], serde_json::json!(true));
    assert_eq!(value["target"], serde_json::json!("Fs_ReadFile"));
    assert!(value["paths"][0]["resolved"]
        .as_str()
        .unwrap()
        .ends_with("a.txt"));

    // deny case via alias (verify + fr)
    let out = Command::new(ACT)
        .args(["verify", "-t", "fr", "--path", "../escape"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(out.status.success());
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["allowed"], serde_json::json!(false));
    assert_eq!(value["code"], serde_json::json!("permission_denied"));
}

#[test]
fn sys_schema_outputs_full_contract() {
    let tmp = tempfile::tempdir().unwrap();
    let output = Command::new(ACT)
        .args(["Sys_Schema"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["name"], serde_json::json!("agent-core-tools"));
    assert!(value["commands"].as_array().unwrap().len() >= 25);
    assert!(value["errors"].as_array().unwrap().len() >= 10);
    for command in value["commands"].as_array().unwrap() {
        assert!(
            command["example"].as_str().is_some(),
            "every command must carry an example: {}",
            command["name"]
        );
    }
    // every command has output variants declared
    let read = value["commands"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "Fs_ReadFile")
        .unwrap();
    assert!(read["outputs"]
        .as_array()
        .unwrap()
        .iter()
        .any(|o| o["success"] == serde_json::json!(true)));
}

#[test]
fn generated_skill_matches_schema_and_has_no_exec_channel() {
    let tmp = tempfile::tempdir().unwrap();
    let schema_out = Command::new(ACT)
        .args(["Sys_Schema"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    let schema: serde_json::Value = serde_json::from_slice(&schema_out.stdout).unwrap();

    let install = Command::new(ACT)
        .args(["Sys_Install", "--project", "--force"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        install.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&install.stderr)
    );
    let skill =
        std::fs::read_to_string(tmp.path().join(".claude/skills/agent-core-tools/SKILL.md"))
            .unwrap();

    assert!(skill.contains("name: agent-core-tools"));
    // The JSON input channel is retired: docs must not teach --input/exec.
    assert!(
        !skill.contains("--input"),
        "SKILL.md must not mention --input"
    );
    assert!(
        !skill.contains("act exec"),
        "SKILL.md must not mention 'act exec'"
    );
    for command in schema["commands"].as_array().unwrap() {
        let name = command["name"].as_str().unwrap();
        assert!(
            skill.contains(&format!("### {name}\n")),
            "SKILL.md missing section for {name}"
        );
        if let Some(example) = command["example"].as_str() {
            if !example.is_empty() {
                assert!(
                    skill.contains(&format!("act {name} {example}")),
                    "SKILL.md example drift for {name}"
                );
            }
        }
    }
    assert!(
        !tmp.path().join(".mcp.json").exists(),
        "install must not write .mcp.json by default"
    );
    assert!(tmp
        .path()
        .join(".claude/skills/agent-core-tools/schema.json")
        .is_file());
    assert!(tmp
        .path()
        .join(".claude/skills/agent-core-tools/mcp.json")
        .is_file());
}

#[test]
fn sys_install_with_mcp_flag_writes_config() {
    let tmp = tempfile::tempdir().unwrap();
    let out = Command::new(ACT)
        .args(["Sys_Install", "--project", "--with-mcp", "--force"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let mcp: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(tmp.path().join(".mcp.json")).unwrap())
            .unwrap();
    assert_eq!(
        mcp["mcpServers"]["act"]["args"][0],
        serde_json::json!("mcp")
    );
}

#[test]
fn sys_package_creates_selfcontained_skill_zip() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("dist");
    let output = Command::new(ACT)
        .args(["Sys_Package", "--out"])
        .arg(&out)
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let staging = out.join("agent-core-tools");
    assert!(staging.join("SKILL.md").is_file());
    assert!(staging.join("schema.json").is_file());
    assert!(staging.join("mcp.json").is_file());
    let exe_name = if cfg!(windows) { "act.exe" } else { "act" };
    assert!(staging.join(exe_name).is_file());

    let zips: Vec<std::path::PathBuf> = std::fs::read_dir(&out)
        .unwrap()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().map(|e| e == "zip").unwrap_or(false))
        .collect();
    assert_eq!(zips.len(), 1, "exactly one zip expected");
    let name = zips[0].file_name().unwrap().to_string_lossy().to_string();
    assert!(name.starts_with("agent-core-tools-"), "zip name: {name}");

    let file = std::fs::File::open(&zips[0]).unwrap();
    let mut archive = zip::ZipArchive::new(file).unwrap();
    let names: Vec<String> = (0..archive.len())
        .map(|i| archive.by_index(i).unwrap().name().to_string())
        .collect();
    for expected in [
        "agent-core-tools/".to_string(),
        "agent-core-tools/SKILL.md".to_string(),
        "agent-core-tools/schema.json".to_string(),
        "agent-core-tools/mcp.json".to_string(),
        format!("agent-core-tools/{exe_name}"),
    ] {
        assert!(
            names.contains(&expected),
            "zip missing {expected}: {names:?}"
        );
    }
}

// ---------- Sys_Serve (MCP stdio, optional channel) ----------

#[test]
fn mcp_stdio_protocol_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("mcp.txt"), "mcp 内容").unwrap();
    let mut child = ChildIo::spawn(&["Sys_Serve"], tmp.path());

    child.send_line(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#);
    let response: serde_json::Value = serde_json::from_str(&child.read_line()).unwrap();
    assert_eq!(
        response["result"]["serverInfo"]["name"],
        serde_json::json!("agent-core-tools")
    );

    child.send_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
    child.send_line(r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#);
    let response: serde_json::Value = serde_json::from_str(&child.read_line()).unwrap();
    assert_eq!(response["id"], serde_json::json!(2));

    child.send_line(r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#);
    let response: serde_json::Value = serde_json::from_str(&child.read_line()).unwrap();
    assert!(response["result"]["tools"].as_array().unwrap().len() >= 25);

    child.send_line(r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"Fs_ReadFile","arguments":{"path":"mcp.txt"}}}"#);
    let response: serde_json::Value = serde_json::from_str(&child.read_line()).unwrap();
    assert_eq!(response["result"]["isError"], serde_json::json!(false));
    let inner: serde_json::Value =
        serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(inner["content"], serde_json::json!("mcp 内容"));

    child.send_line(r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"Fs_ReadFile","arguments":{"path":"../../etc/passwd"}}}"#);
    let response: serde_json::Value = serde_json::from_str(&child.read_line()).unwrap();
    assert_eq!(response["result"]["isError"], serde_json::json!(true));

    child.send_line(r#"{"jsonrpc":"2.0","id":6,"method":"resources/list"}"#);
    let response: serde_json::Value = serde_json::from_str(&child.read_line()).unwrap();
    assert_eq!(response["error"]["code"], serde_json::json!(-32601));
}

#[test]
fn mcp_alias_also_serves() {
    // `act mcp` remains as a Sys_Serve alias for muscle memory.
    let tmp = tempfile::tempdir().unwrap();
    let mut child = ChildIo::spawn(&["mcp"], tmp.path());
    child.send_line(r#"{"jsonrpc":"2.0","id":1,"method":"ping"}"#);
    let response: serde_json::Value = serde_json::from_str(&child.read_line()).unwrap();
    assert_eq!(response["result"], serde_json::json!({}));
}

// ---------- Global flags ----------

#[test]
fn global_config_flag_before_command() {
    let tmp = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("a.txt"), b"x").unwrap();
    // `--root` adds tmp as an extra sandbox root; cwd stays the primary root.
    let abs = tmp.path().join("a.txt").to_str().unwrap().to_string();
    let root = tmp.path().to_str().unwrap().to_string();
    let out = Command::new(ACT)
        .args(["--root", &root, "Fs_ReadFile", "--path", &abs])
        .current_dir(cwd.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["content"], serde_json::json!("x"));
}
