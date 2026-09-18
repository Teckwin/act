//! MCP stdio server: JSON-RPC 2.0 over line-delimited stdin/stdout.
//!
//! Supported methods: initialize, ping, tools/list, tools/call. All output
//! goes to stdout; logs go to stderr so the protocol channel stays clean.

use act_kernel::error::ActResult;
use act_kernel::{CommandManager, InvokeMode};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &["2024-11-05", "2025-03-26", "2025-06-18"];
const SERVER_NAME: &str = "agent-core-tools";

pub async fn serve(manager: CommandManager) -> ActResult<()> {
    let stdin = tokio::io::stdin();
    let mut reader = BufReader::new(stdin);
    let mut stdout = tokio::io::stdout();
    let mut line = String::new();

    loop {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .await
            .map_err(|e| act_kernel::ActError::Other(format!("stdin: {e}")))?;
        if n == 0 {
            // EOF: client closed the channel.
            return Ok(());
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let message: Value = match serde_json::from_str(trimmed) {
            Ok(value) => value,
            Err(err) => {
                write_jsonrpc_error(
                    &mut stdout,
                    Value::Null,
                    -32700,
                    &format!("parse error: {err}"),
                )
                .await;
                continue;
            }
        };

        let id = message.get("id").cloned();
        let method = message
            .get("method")
            .and_then(|m| m.as_str())
            .unwrap_or("")
            .to_string();

        // Notifications (no id) never get a response.
        let Some(id) = id else {
            continue;
        };

        let outcome: Result<Value, (i64, String)> = match method.as_str() {
            "initialize" => Ok(initialize_result(&message)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(tools_list(&manager)),
            "tools/call" => tools_call(&manager, &message).await,
            other => Err((-32601, format!("method not found: {other}"))),
        };

        match outcome {
            Ok(result) => write_jsonrpc_result(&mut stdout, id, result).await,
            Err((code, message)) => write_jsonrpc_error(&mut stdout, id, code, &message).await,
        }
    }
}

fn initialize_result(message: &Value) -> Value {
    let requested = message
        .pointer("/params/protocolVersion")
        .and_then(|v| v.as_str())
        .unwrap_or("2024-11-05");
    let version = if SUPPORTED_PROTOCOL_VERSIONS.contains(&requested) {
        requested
    } else {
        "2024-11-05"
    };
    json!({
        "protocolVersion": version,
        "capabilities": {
            "tools": { "listChanged": false }
        },
        "serverInfo": {
            "name": SERVER_NAME,
            "version": env!("CARGO_PKG_VERSION")
        }
    })
}

fn tools_list(manager: &CommandManager) -> Value {
    json!({
        "tools": manager
            .list()
            .into_iter()
            .map(|info| json!({
                "name": info.name,
                "description": info.description,
                "inputSchema": info.input_schema,
            }))
            .collect::<Vec<_>>()
    })
}

async fn tools_call(manager: &CommandManager, message: &Value) -> Result<Value, (i64, String)> {
    let name = message
        .pointer("/params/name")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let arguments = message
        .pointer("/params/arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if name.is_empty() {
        return Err((-32602, "params.name is required".into()));
    }
    match manager.execute(&name, arguments, InvokeMode::Mcp).await {
        Ok(result) => {
            let text = serde_json::to_string_pretty(&result).unwrap_or_else(|_| result.to_string());
            Ok(json!({
                "content": [{ "type": "text", "text": text }],
                "isError": false
            }))
        }
        Err(err) => {
            let payload = json!({
                "code": err.code(),
                "message": err.to_string(),
            });
            Ok(json!({
                "content": [{ "type": "text", "text": payload.to_string() }],
                "isError": true
            }))
        }
    }
}

async fn write_jsonrpc_result(stdout: &mut tokio::io::Stdout, id: Value, result: Value) {
    let response = json!({ "jsonrpc": "2.0", "id": id, "result": result });
    if let Ok(text) = serde_json::to_string(&response) {
        let _ = stdout.write_all(text.as_bytes()).await;
        let _ = stdout.write_all(b"\n").await;
        let _ = stdout.flush().await;
    }
}

async fn write_jsonrpc_error(stdout: &mut tokio::io::Stdout, id: Value, code: i64, message: &str) {
    let response = json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message }
    });
    if let Ok(text) = serde_json::to_string(&response) {
        let _ = stdout.write_all(text.as_bytes()).await;
        let _ = stdout.write_all(b"\n").await;
        let _ = stdout.flush().await;
    }
}
