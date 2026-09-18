//! End-to-end tests: CLI flags mode (primary), exec JSON channel, MCP stdio
//! protocol, install and packaging.

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

// ---------- CLI flags mode (primary interface) ----------

#[test]
fn alias_short_command_and_short_flags() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("alias.txt"), "alias ok 中文").unwrap();
    // `fr` (command alias) + `-p` (param short alias) ≡ Fs_ReadFile --path
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

    // combined short flags: fw -p a.txt -c 内容
    let out = Command::new(ACT)
        .args(["fw", "-p", "new.txt", "-c", "短参写入"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["command"], serde_json::json!("Fs_WriteFile"));
    let out = Command::new(ACT)
        .args(["fr", "-p", "new.txt"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["content"], serde_json::json!("短参写入"));
}

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
    // write (gbk)
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
    // edit with sugar
    let out = Command::new(ACT)
        .args(["Fs_EditFile", "--path", "cn.txt", "--edit", "旧=>新"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["replacements"], serde_json::json!(1));
    assert_eq!(value["encoding"], serde_json::json!("gbk"));
    // read back
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
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("--path"), "stderr: {stderr}");
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
fn exec_json_channel_still_works() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("a.txt"), "x").unwrap();
    let out = Command::new(ACT)
        .args(["exec", "Fs_ReadFile", "--input", r#"{"path":"a.txt"}"#])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(out.status.success());
    let value: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(value["content"], serde_json::json!("x"));
}

#[test]
fn cli_list_shows_all_commands() {
    let tmp = tempfile::tempdir().unwrap();
    let output = Command::new(ACT)
        .args(["list", "--json"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let count = value.as_array().unwrap().len();
    assert!(count >= 19, "expected >= 19 commands, got {count}");
}

#[test]
fn cli_verify_dry_run() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("a.txt"), b"x").unwrap();
    let output = Command::new(ACT)
        .args(["verify", "Fs_ReadFile", "--input", r#"{"path":"a.txt"}"#])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["allowed"], serde_json::json!(true));
}

// ---------- Contract generation & consistency ----------

#[test]
fn cli_schema_outputs_full_contract() {
    let tmp = tempfile::tempdir().unwrap();
    let output = Command::new(ACT)
        .args(["schema"])
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
    assert!(value["commands"].as_array().unwrap().len() >= 19);
    assert!(value["errors"].as_array().unwrap().len() >= 10);
    for command in value["commands"].as_array().unwrap() {
        assert!(
            command["example"].as_str().is_some(),
            "every command must carry an example: {}",
            command["name"]
        );
    }
}

#[test]
fn generated_skill_matches_schema() {
    let tmp = tempfile::tempdir().unwrap();
    let schema_out = Command::new(ACT)
        .args(["schema"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    let schema: serde_json::Value = serde_json::from_slice(&schema_out.stdout).unwrap();

    let install = Command::new(ACT)
        .args(["install", "--project", "--force"])
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
    assert!(skill.contains("Fs_ReadFile"));
    // Every command in the schema must appear as a documented section.
    for command in schema["commands"].as_array().unwrap() {
        let name = command["name"].as_str().unwrap();
        let section = format!("### {name}\n");
        assert!(
            skill.contains(&section),
            "SKILL.md missing section for {name}"
        );
        // The documented example must match the schema example.
        let example = command["example"].as_str().unwrap();
        assert!(
            skill.contains(&format!("act {name} {example}")),
            "SKILL.md example drift for {name}"
        );
    }
    // No MCP registration by default.
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
fn install_with_mcp_flag_writes_config() {
    let tmp = tempfile::tempdir().unwrap();
    let out = Command::new(ACT)
        .args(["install", "--project", "--with-mcp", "--force"])
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
    assert!(
        mcp["mcpServers"]["act"]["command"]
            .as_str()
            .unwrap()
            .contains("act"),
        "command must point at this binary"
    );
}

#[test]
fn package_creates_selfcontained_skill_zip() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("dist");
    let output = Command::new(ACT)
        .args(["package", "--out"])
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

// ---------- MCP stdio (optional integration channel) ----------

#[test]
fn mcp_stdio_protocol_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("mcp.txt"), "mcp 内容").unwrap();
    let mut child = ChildIo::spawn(&["mcp"], tmp.path());

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
    assert!(response["result"]["tools"].as_array().unwrap().len() >= 19);

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
