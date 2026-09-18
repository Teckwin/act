//! Integration tests: local HTTP server fixtures, SSRF-safe fetch pipeline,
//! search merge, and the research pipeline end to end.

use std::sync::Arc;

use act_kernel::error::ActResult;
use act_kernel::{ActConfig, CommandManager, InvokeMode, SandboxContext};
use act_web::commands::research::run_research;
use act_web::commands::search::run_search;
use act_web::engines::SearchEngine;
use async_trait::async_trait;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

// ---------- Minimal local HTTP server ----------

fn route(path: &str) -> (u16, &'static str, Vec<u8>) {
    match path {
        "/page" => (
            200,
            "text/html; charset=utf-8",
            r#"<html><head><title>T</title></head><body>
                <nav>nav junk</nav>
                <h1>Rust 工具箱</h1><p>Agent Core Tools 是一个内核化的工具库。Cross-platform tools kernel.</p>
                </body></html>"#
                .as_bytes()
                .to_vec(),
        ),
        "/page2" => (
            200,
            "text/html; charset=utf-8",
            r#"<html><body><h1>Second Source</h1><p>另一个来源的内容，关于 sandbox 设计。</p></body></html>"#
                .as_bytes()
                .to_vec(),
        ),
        "/redirect" => (302, "text/plain", b"/page".to_vec()),
        "/redirect-loop" => (302, "text/plain", b"/redirect-loop".to_vec()),
        "/json" => (200, "application/json", br#"{"name":"act","version":1}"#.to_vec()),
        "/big" => (200, "text/plain", vec![b'x'; 300_000]),
        _ => (404, "text/plain", b"not found".to_vec()),
    }
}

async fn handle_conn(mut socket: TcpStream) {
    let mut data = Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        match socket.read(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                data.extend_from_slice(&buf[..n]);
                if data.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
        }
    }
    let head = String::from_utf8_lossy(&data);
    let path = head.split_whitespace().nth(1).unwrap_or("/").to_string();
    let (status, ctype, body) = route(&path);
    let head = if status == 302 {
        let target = String::from_utf8_lossy(&body).to_string();
        format!(
            "HTTP/1.1 302 Found\r\nLocation: {target}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        )
    } else {
        let reason = if status == 200 { "OK" } else { "Not Found" };
        format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: {ctype}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
    };
    let _ = socket.write_all(head.as_bytes()).await;
    if status != 302 {
        let _ = socket.write_all(&body).await;
    }
}

async fn start_server() -> (u16, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let handle = tokio::spawn(async move {
        loop {
            let Ok((socket, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(handle_conn(socket));
        }
    });
    (port, handle)
}

// ---------- Fixtures ----------

struct StubEngine {
    base: String,
}

#[async_trait]
impl SearchEngine for StubEngine {
    fn name(&self) -> &'static str {
        "stub"
    }
    async fn search(
        &self,
        _ctx: &SandboxContext,
        _query: &str,
        _count: usize,
    ) -> ActResult<Vec<act_web::engines::SearchHit>> {
        Ok(vec![
            act_web::engines::SearchHit {
                title: "Rust 工具箱".into(),
                url: format!("{}/page", self.base),
                snippet: "Agent Core Tools 工具库".into(),
            },
            act_web::engines::SearchHit {
                title: "Second Source".into(),
                url: format!("{}/page2", self.base),
                snippet: "sandbox 设计".into(),
            },
        ])
    }
}

fn sandbox_in(dir: &std::path::Path, port: u16) -> Arc<SandboxContext> {
    let mut cfg = ActConfig {
        roots: vec![dir.to_path_buf()],
        ..Default::default()
    };
    cfg.url.allow_private_ips = true; // local test server
    let _ = port;
    Arc::new(SandboxContext::new(
        Arc::new(cfg.clone()),
        Arc::new(act_kernel::guard::path_guard::PathGuard::new(vec![dir.to_path_buf()]).unwrap()),
        act_kernel::guard::url_guard::UrlGuard::new(cfg.url.clone()),
        act_kernel::guard::protect_guard::ProtectGuard::from_config(&[], false),
    ))
}
async fn manager_with_local_server(
    dir: &std::path::Path,
) -> (CommandManager, u16, tokio::task::JoinHandle<()>) {
    let (port, server) = start_server().await;
    let mut cfg = ActConfig {
        roots: vec![dir.to_path_buf()],
        ..Default::default()
    };
    cfg.url.allow_private_ips = true;
    let manager = CommandManager::new(cfg).unwrap();
    act_web::register_all(&manager).unwrap();
    (manager, port, server)
}

// ---------- Tests ----------

#[tokio::test]
async fn fetch_markdown_conversion() {
    let tmp = tempfile::tempdir().unwrap();
    let (m, port, _server) = manager_with_local_server(tmp.path()).await;
    let out = m
        .execute(
            "Web_Fetch",
            json!({"urls": [format!("http://127.0.0.1:{port}/page")]}),
            InvokeMode::Cli,
        )
        .await
        .unwrap();
    let item = &out["results"][0];
    assert_eq!(item["ok"], json!(true), "item: {item}");
    let content = item["content"].as_str().unwrap();
    assert!(content.contains("# Rust 工具箱"), "content: {content}");
    assert!(content.contains("Agent Core Tools"));
    assert!(!content.contains("nav junk"));
    assert_eq!(
        item["content_type"].as_str().unwrap(),
        "text/html; charset=utf-8"
    );
}

#[tokio::test]
async fn fetch_follows_redirects() {
    let tmp = tempfile::tempdir().unwrap();
    let (m, port, _server) = manager_with_local_server(tmp.path()).await;
    let out = m
        .execute(
            "Web_Fetch",
            json!({"urls": [format!("http://127.0.0.1:{port}/redirect")]}),
            InvokeMode::Cli,
        )
        .await
        .unwrap();
    let item = &out["results"][0];
    assert_eq!(item["ok"], json!(true), "item: {item}");
    assert!(item["final_url"].as_str().unwrap().ends_with("/page"));
    assert_eq!(item["redirects"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn fetch_redirect_loop_bounded() {
    let tmp = tempfile::tempdir().unwrap();
    let (m, port, _server) = manager_with_local_server(tmp.path()).await;
    let out = m
        .execute(
            "Web_Fetch",
            json!({"urls": [format!("http://127.0.0.1:{port}/redirect-loop")]}),
            InvokeMode::Cli,
        )
        .await
        .unwrap();
    let item = &out["results"][0];
    assert_eq!(item["ok"], json!(false));
    assert!(
        item["error"].as_str().unwrap().contains("redirects"),
        "err: {item}"
    );
}

#[tokio::test]
async fn fetch_404_isolated_per_item() {
    let tmp = tempfile::tempdir().unwrap();
    let (m, port, _server) = manager_with_local_server(tmp.path()).await;
    let out = m
        .execute(
            "Web_Fetch",
            json!({"urls": [format!("http://127.0.0.1:{port}/page"), format!("http://127.0.0.1:{port}/missing")]}),
            InvokeMode::Cli,
        )
        .await
        .unwrap();
    assert_eq!(out["summary"]["succeeded"], json!(1));
    assert_eq!(out["summary"]["failed"], json!(1));
    assert!(out["results"][1]["error"].as_str().unwrap().contains("404"));
}

#[tokio::test]
async fn fetch_json_format() {
    let tmp = tempfile::tempdir().unwrap();
    let (m, port, _server) = manager_with_local_server(tmp.path()).await;
    let out = m
        .execute(
            "Web_Fetch",
            json!({"urls": [format!("http://127.0.0.1:{port}/json")], "format": "json"}),
            InvokeMode::Cli,
        )
        .await
        .unwrap();
    let content = out["results"][0]["content"].as_str().unwrap();
    assert!(content.contains("\"name\": \"act\""));
}

#[tokio::test]
async fn fetch_respects_max_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    let (m, port, _server) = manager_with_local_server(tmp.path()).await;
    let out = m
        .execute(
            "Web_Fetch",
            json!({"urls": [format!("http://127.0.0.1:{port}/big")], "max_bytes": 1024}),
            InvokeMode::Cli,
        )
        .await
        .unwrap();
    let item = &out["results"][0];
    assert_eq!(item["ok"], json!(true));
    assert_eq!(item["truncated"], json!(true));
    assert_eq!(item["bytes"], json!(1024));
}

#[tokio::test]
async fn fetch_private_ip_denied_by_default() {
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = ActConfig {
        roots: vec![tmp.path().to_path_buf()],
        ..Default::default()
    };
    let m = CommandManager::new(cfg.clone()).unwrap();
    act_web::register_all(&m).unwrap();
    let err = m
        .execute(
            "Web_Fetch",
            json!({"urls": ["http://127.0.0.1:1/x"]}),
            InvokeMode::Cli,
        )
        .await
        .unwrap_err();
    assert!(
        matches!(err, act_kernel::ActError::PermissionDenied { .. }),
        "{err:?}"
    );
    cfg.url.allow_private_ips = true;
}

#[tokio::test]
async fn search_merge_via_run_search() {
    let tmp = tempfile::tempdir().unwrap();
    let (port, _server) = start_server().await;
    let ctx = sandbox_in(tmp.path(), port);
    let stub: Arc<dyn SearchEngine> = Arc::new(StubEngine {
        base: format!("http://127.0.0.1:{port}"),
    });
    let out = run_search(&ctx, &["工具库".to_string()], &[stub], 10).await;
    let results = out["queries"][0]["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0]["rank"], json!(1));
}

#[tokio::test]
async fn research_pipeline_end_to_end() {
    let tmp = tempfile::tempdir().unwrap();
    let (port, _server) = start_server().await;
    let ctx = sandbox_in(tmp.path(), port);
    let stub: Arc<dyn SearchEngine> = Arc::new(StubEngine {
        base: format!("http://127.0.0.1:{port}"),
    });
    let out = run_research(&ctx, "agent tools 内核设计", &[stub], 4, 10)
        .await
        .unwrap();
    let report = out["report"].as_str().unwrap();
    assert!(report.contains("# Research: agent tools 内核设计"));
    assert!(report.contains("[1] Rust 工具箱"));
    assert!(report.contains("http://127.0.0.1"));
    let sources = out["sources"].as_array().unwrap();
    assert_eq!(sources.len(), 2);
    assert!(!sources[0]["summary"].as_str().unwrap().is_empty());
}
