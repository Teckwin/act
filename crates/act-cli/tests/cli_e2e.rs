//! End-to-end tests: CLI subprocess + MCP stdio protocol subprocess.

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

#[test]
fn cli_exec_reads_file() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("hello.txt"), "你好 world").unwrap();

    let output = Command::new(ACT)
        .args([
            "exec",
            "Fs_ReadFile",
            "--input",
            r#"{"paths":["hello.txt"]}"#,
        ])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout.clone()).unwrap();
    let value: serde_json::Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(
        value["results"][0]["content"],
        serde_json::json!("你好 world")
    );
    assert_eq!(value["results"][0]["encoding"], serde_json::json!("utf-8"));
}

#[test]
fn cli_exec_traversal_exit_code_2() {
    let tmp = tempfile::tempdir().unwrap();
    let output = Command::new(ACT)
        .args(["exec", "Fs_ReadFile", "--input", r#"{"paths":["../../x"]}"#])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("permission_denied"), "stderr: {stderr}");
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
    assert!(count >= 18, "expected >= 18 commands, got {count}");
    let names: Vec<&str> = value
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    for expected in [
        "Fs_ReadFile",
        "Fs_WriteFile",
        "Fs_GrepFile",
        "Web_Fetch",
        "Web_Search",
        "Web_Research",
    ] {
        assert!(names.contains(&expected), "missing {expected}");
    }
}

#[test]
fn cli_verify_dry_run() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("a.txt"), b"x").unwrap();
    let output = Command::new(ACT)
        .args(["verify", "Fs_ReadFile", "--input", r#"{"paths":["a.txt"]}"#])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["allowed"], serde_json::json!(true));
}

#[test]
fn mcp_stdio_protocol_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("mcp.txt"), "mcp 内容").unwrap();
    let mut child = ChildIo::spawn(&["mcp"], tmp.path());

    // initialize
    child.send_line(r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"0"}}}"#);
    let response: serde_json::Value = serde_json::from_str(&child.read_line()).unwrap();
    assert_eq!(response["id"], serde_json::json!(1));
    assert_eq!(
        response["result"]["protocolVersion"],
        serde_json::json!("2024-11-05")
    );
    assert_eq!(
        response["result"]["serverInfo"]["name"],
        serde_json::json!("agent-core-tools")
    );

    // notification: no response expected; follow with a ping to sync.
    child.send_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
    child.send_line(r#"{"jsonrpc":"2.0","id":2,"method":"ping"}"#);
    let response: serde_json::Value = serde_json::from_str(&child.read_line()).unwrap();
    assert_eq!(response["id"], serde_json::json!(2));
    assert_eq!(response["result"], serde_json::json!({}));

    // tools/list
    child.send_line(r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#);
    let response: serde_json::Value = serde_json::from_str(&child.read_line()).unwrap();
    let tools = response["result"]["tools"].as_array().unwrap().clone();
    assert!(tools.len() >= 18, "tools: {}", tools.len());
    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"Fs_ReadFile"));

    // tools/call happy path
    child.send_line(r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"Fs_ReadFile","arguments":{"paths":["mcp.txt"]}}}"#);
    let response: serde_json::Value = serde_json::from_str(&child.read_line()).unwrap();
    assert_eq!(response["result"]["isError"], serde_json::json!(false));
    let text = response["result"]["content"][0]["text"].as_str().unwrap();
    let inner: serde_json::Value = serde_json::from_str(text).unwrap();
    assert_eq!(
        inner["results"][0]["content"],
        serde_json::json!("mcp 内容")
    );

    // tools/call permission denial -> isError true
    child.send_line(r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"Fs_ReadFile","arguments":{"paths":["../../etc/passwd"]}}}"#);
    let response: serde_json::Value = serde_json::from_str(&child.read_line()).unwrap();
    assert_eq!(response["result"]["isError"], serde_json::json!(true));

    // unknown method -> JSON-RPC error
    child.send_line(r#"{"jsonrpc":"2.0","id":6,"method":"resources/list"}"#);
    let response: serde_json::Value = serde_json::from_str(&child.read_line()).unwrap();
    assert_eq!(response["error"]["code"], serde_json::json!(-32601));

    // parse error -> -32700
    child.send_line("{not json");
    let response: serde_json::Value = serde_json::from_str(&child.read_line()).unwrap();
    assert_eq!(response["error"]["code"], serde_json::json!(-32700));
}

#[test]
fn install_project_writes_mcp_and_skill() {
    let tmp = tempfile::tempdir().unwrap();
    let output = Command::new(ACT)
        .args(["install", "--project", "--force"])
        .current_dir(tmp.path())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let mcp_path = tmp.path().join(".mcp.json");
    assert!(mcp_path.is_file());
    let mcp: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&mcp_path).unwrap()).unwrap();
    assert_eq!(
        mcp["mcpServers"]["act"]["args"][0],
        serde_json::json!("mcp")
    );

    let skill_path = tmp.path().join(".claude/skills/agent-core-tools/SKILL.md");
    assert!(skill_path.is_file());
    let skill = std::fs::read_to_string(&skill_path).unwrap();
    assert!(skill.contains("name: agent-core-tools"));
    assert!(skill.contains("Fs_ReadFile"));
}
