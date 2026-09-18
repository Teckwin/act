//! Integration tests: single-target semantics, security matrix, encoding
//! roundtrips, trash behavior (v0.2 CLI-first parameter surface).

use act_kernel::error::ActError;
use act_kernel::{ActConfig, CommandManager, InvokeMode};
use serde_json::{json, Value};

fn manager_in(dir: &std::path::Path) -> CommandManager {
    let cfg = ActConfig {
        roots: vec![dir.to_path_buf()],
        ..Default::default()
    };
    let m = CommandManager::new(cfg).expect("manager");
    act_fs::register_all(&m).expect("register fs commands");
    m
}

async fn read(m: &CommandManager, path: &str) -> Result<Value, ActError> {
    m.execute("Fs_ReadFile", json!({ "path": path }), InvokeMode::Cli)
        .await
}

async fn exec_cmd(m: &CommandManager, command: &str, params: Value) -> Result<Value, ActError> {
    m.execute(command, params, InvokeMode::Cli).await
}

// ---------- Single-target semantics ----------

#[tokio::test]
async fn read_returns_flat_envelope() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("a.txt"), "line1\nline2").unwrap();
    let m = manager_in(tmp.path());
    let out = read(&m, "a.txt").await.unwrap();
    assert_eq!(out["ok"], json!(true));
    assert_eq!(out["command"], json!("Fs_ReadFile"));
    assert_eq!(out["path"], json!("a.txt"));
    assert_eq!(out["content"], json!("line1\nline2"));
    assert_eq!(out["encoding"], json!("utf-8"));
    assert!(
        out.get("results").is_none(),
        "single-target commands must not wrap results[]"
    );
}

#[tokio::test]
async fn read_line_slicing() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("l.txt"), "l1\nl2\nl3\nl4\nl5").unwrap();
    let m = manager_in(tmp.path());
    let out = exec_cmd(
        &m,
        "Fs_ReadFile",
        json!({"path": "l.txt", "offset": 1, "limit": 2}),
    )
    .await
    .unwrap();
    assert_eq!(out["content"], json!("l2\nl3"));
    assert_eq!(out["line_start"], json!(1));
    assert_eq!(out["total_lines"], json!(5));
}

#[tokio::test]
async fn write_single_and_read_back() {
    let tmp = tempfile::tempdir().unwrap();
    let m = manager_in(tmp.path());
    let out = exec_cmd(
        &m,
        "Fs_WriteFile",
        json!({"path": "cn.txt", "content": "中文内容", "encoding": "gbk"}),
    )
    .await
    .unwrap();
    assert_eq!(out["bytes"], json!(8));
    assert_eq!(out["encoding"], json!("gbk"));
    assert!(out.get("results").is_none());
    let out = read(&m, "cn.txt").await.unwrap();
    assert_eq!(out["content"], json!("中文内容"));
    assert_eq!(out["encoding"], json!("gbk"));
}

#[tokio::test]
async fn edit_replaces_and_preserves_gbk() {
    let tmp = tempfile::tempdir().unwrap();
    let m = manager_in(tmp.path());
    exec_cmd(
        &m,
        "Fs_WriteFile",
        json!({"path": "e.txt", "content": "旧的内容", "encoding": "gbk"}),
    )
    .await
    .unwrap();
    let out = exec_cmd(
        &m,
        "Fs_EditFile",
        json!({"path": "e.txt", "edits": [{"old": "旧", "new": "新"}]}),
    )
    .await
    .unwrap();
    assert_eq!(out["replacements"], json!(1));
    assert_eq!(out["encoding"], json!("gbk"));
    let out = read(&m, "e.txt").await.unwrap();
    assert_eq!(out["content"], json!("新的内容"));
}

#[tokio::test]
async fn edit_ambiguity_rejected_file_untouched() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("amb.txt"), "a a a").unwrap();
    let m = manager_in(tmp.path());
    let err = exec_cmd(
        &m,
        "Fs_EditFile",
        json!({"path": "amb.txt", "edits": [{"old": "a", "new": "b"}]}),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, ActError::Execution { .. }));
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("amb.txt")).unwrap(),
        "a a a"
    );
}

#[tokio::test]
async fn move_copy_single_pair_flat_output() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(tmp.path().join("src/f.txt"), b"x").unwrap();
    let m = manager_in(tmp.path());

    let out = exec_cmd(&m, "Fs_MoveDir", json!({"from": "src", "to": "renamed"}))
        .await
        .unwrap();
    assert_eq!(out["op"], json!("moved"));
    assert_eq!(out["from"], json!("src"));
    assert!(tmp.path().join("renamed/f.txt").is_file());

    let out = exec_cmd(&m, "Fs_CopyDir", json!({"from": "renamed", "to": "copy"}))
        .await
        .unwrap();
    assert_eq!(out["op"], json!("copied"));
    assert!(tmp.path().join("copy/f.txt").is_file());

    // dest exists without overwrite -> rejected
    let err = exec_cmd(&m, "Fs_CopyDir", json!({"from": "copy", "to": "renamed"}))
        .await
        .unwrap_err();
    assert!(matches!(err, ActError::Execution { .. }));
}

#[tokio::test]
async fn move_into_own_child_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("parent/child")).unwrap();
    let m = manager_in(tmp.path());
    let err = exec_cmd(
        &m,
        "Fs_MoveDir",
        json!({"from": "parent", "to": "parent/child/sub"}),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, ActError::InvalidParams { .. }));
}

#[tokio::test]
async fn list_and_info_single_dir() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("x.txt"), b"1").unwrap();
    std::fs::create_dir_all(tmp.path().join("sub")).unwrap();
    let m = manager_in(tmp.path());

    let out = exec_cmd(&m, "Fs_ListDir", json!({"path": ".", "depth": 2}))
        .await
        .unwrap();
    assert_eq!(out["count"], json!(2));
    assert!(out.get("results").is_none());

    let out = exec_cmd(&m, "Fs_FileInfo", json!({"path": "x.txt"}))
        .await
        .unwrap();
    assert_eq!(out["size"], json!(1));
    assert_eq!(out["is_file"], json!(true));
}

#[tokio::test]
async fn batch_create_and_mkdir_keep_envelope() {
    let tmp = tempfile::tempdir().unwrap();
    let m = manager_in(tmp.path());
    let out = exec_cmd(
        &m,
        "Fs_CreateFile",
        json!({"paths": ["a.txt", "b.txt", "missing-dir/c.txt"]}),
    )
    .await
    .unwrap();
    assert_eq!(out["summary"]["succeeded"], json!(3));
    assert!(out["results"].as_array().unwrap().len() == 3);

    let out = exec_cmd(&m, "Fs_CreateDir", json!({"paths": ["d1/d2", "d3"]}))
        .await
        .unwrap();
    assert_eq!(out["summary"]["succeeded"], json!(2));
    assert!(tmp.path().join("d1/d2").is_dir());
}

#[tokio::test]
async fn create_partial_failure_isolated() {
    let tmp = tempfile::tempdir().unwrap();
    let m = manager_in(tmp.path());
    // One valid + one escaping path: security failure rejects the whole call.
    let err = exec_cmd(
        &m,
        "Fs_CreateFile",
        json!({"paths": ["ok.txt", "../escape.txt"]}),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, ActError::PermissionDenied { .. }));
}

// ---------- Security matrix ----------

#[tokio::test]
async fn read_traversal_denied() {
    let tmp = tempfile::tempdir().unwrap();
    let m = manager_in(tmp.path());
    for bad in ["../../x", "a/../../..", "/etc/passwd"] {
        let err = read(&m, bad).await.unwrap_err();
        assert!(
            matches!(err, ActError::PermissionDenied { .. }),
            "{bad}: {err:?}"
        );
    }
}

#[tokio::test]
async fn write_traversal_denied() {
    let tmp = tempfile::tempdir().unwrap();
    let m = manager_in(tmp.path());
    for bad in ["../out.txt", "a/b/../../../out.txt"] {
        let err = exec_cmd(&m, "Fs_WriteFile", json!({"path": bad, "content": "x"}))
            .await
            .unwrap_err();
        assert!(
            matches!(err, ActError::PermissionDenied { .. }),
            "{bad}: {err:?}"
        );
    }
}

#[tokio::test]
async fn protected_paths_denied_across_commands() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join(".git")).unwrap();
    std::fs::write(tmp.path().join(".git/config"), b"[core]").unwrap();
    std::fs::write(tmp.path().join(".env"), b"SECRET=1").unwrap();
    let m = manager_in(tmp.path());

    let cases: Vec<(&str, Value)> = vec![
        ("Fs_ReadFile", json!({"path": ".git/config"})),
        ("Fs_ReadFile", json!({"path": ".env"})),
        (
            "Fs_WriteFile",
            json!({"path": ".git/config", "content": "x"}),
        ),
        ("Fs_AppendFile", json!({"path": ".env", "content": "x"})),
        ("Fs_RemoveFile", json!({"path": ".env"})),
        ("Fs_RemoveDir", json!({"path": ".git", "recursive": true})),
        ("Fs_MoveFile", json!({"from": ".env", "to": "leak.txt"})),
        (
            "Fs_EditFile",
            json!({"path": ".env", "edits": [{"old": "S", "new": "X"}]}),
        ),
    ];
    for (command, params) in cases {
        let err = exec_cmd(&m, command, params).await.unwrap_err();
        assert!(
            matches!(
                err,
                ActError::PermissionDenied {
                    guard: "ProtectGuard",
                    ..
                }
            ),
            "{command}: {err:?}"
        );
    }
}

#[tokio::test]
async fn remove_root_denied() {
    let tmp = tempfile::tempdir().unwrap();
    let m = manager_in(tmp.path());
    let err = exec_cmd(&m, "Fs_RemoveDir", json!({"path": ".", "recursive": true}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("sandbox root"), "got: {err:?}");
    assert!(tmp.path().is_dir());
}

#[tokio::test]
async fn non_recursive_remove_dir_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("d")).unwrap();
    std::fs::write(tmp.path().join("d/f.txt"), b"x").unwrap();
    let m = manager_in(tmp.path());
    let err = exec_cmd(&m, "Fs_RemoveDir", json!({"path": "d"}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("recursive"));
    assert!(tmp.path().join("d/f.txt").exists());
}

// ---------- Trash ----------

#[tokio::test]
async fn remove_file_goes_to_trash_and_readable_back() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("t.txt"), b"data").unwrap();
    let m = manager_in(tmp.path());
    let out = exec_cmd(&m, "Fs_RemoveFile", json!({"path": "t.txt"}))
        .await
        .unwrap();
    assert_eq!(out["mode"], json!("trash"));
    let trash = out["trash_path"].as_str().unwrap().to_string();
    assert!(trash.starts_with(".act/trash/"), "trash path: {trash}");
    assert!(tmp.path().join(&trash).is_file());
    assert!(!tmp.path().join("t.txt").exists());
    // overflow/trash are agent-readable.
    let out = read(&m, &trash).await.unwrap();
    assert_eq!(out["content"], json!("data"));
}

#[tokio::test]
async fn permanent_mode_configurable() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("p.txt"), b"data").unwrap();
    let mut cfg = ActConfig {
        roots: vec![tmp.path().to_path_buf()],
        ..Default::default()
    };
    cfg.fs.delete_mode = act_kernel::config::DeleteMode::Permanent;
    let m = CommandManager::new(cfg).unwrap();
    act_fs::register_all(&m).unwrap();
    let out = exec_cmd(&m, "Fs_RemoveFile", json!({"path": "p.txt"}))
        .await
        .unwrap();
    assert_eq!(out["mode"], json!("permanent"));
    assert!(!tmp.path().join("p.txt").exists());
}

// ---------- Encoding ----------

#[tokio::test]
async fn read_gbk_file() {
    let tmp = tempfile::tempdir().unwrap();
    let gbk_bytes: Vec<u8> = [0xC4, 0xE3, 0xBA, 0xC3, 0xA3, 0xAC, 0xCA, 0xC0, 0xBD, 0xE7].to_vec();
    std::fs::write(tmp.path().join("cn.txt"), &gbk_bytes).unwrap();
    let m = manager_in(tmp.path());
    let out = read(&m, "cn.txt").await.unwrap();
    assert_eq!(out["encoding"], json!("gbk"));
    assert_eq!(out["content"], json!("你好，世界"));
}

#[tokio::test]
async fn utf16_bom_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let m = manager_in(tmp.path());
    exec_cmd(
        &m,
        "Fs_WriteFile",
        json!({"path": "u16.txt", "content": "hello 世界", "encoding": "utf-16le"}),
    )
    .await
    .unwrap();
    let out = read(&m, "u16.txt").await.unwrap();
    assert_eq!(out["encoding"], json!("utf-16le"));
    assert_eq!(out["content"], json!("hello 世界"));
}

// ---------- Find / Grep ----------

#[tokio::test]
async fn find_by_glob_with_chinese_names() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("src")).unwrap();
    std::fs::write(tmp.path().join("src/工具类.rs"), b"fn main() {}").unwrap();
    std::fs::write(tmp.path().join("src/main.rs"), b"fn main() {}").unwrap();
    let m = manager_in(tmp.path());
    let out = exec_cmd(
        &m,
        "Fs_FindFile",
        json!({"root": ".", "patterns": ["*.rs"]}),
    )
    .await
    .unwrap();
    assert_eq!(out["count"], json!(2));
    let paths: Vec<&str> = out["matches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["path"].as_str().unwrap())
        .collect();
    assert!(paths.iter().any(|p| p.contains("工具类.rs")));
}

#[tokio::test]
async fn grep_chinese_content_in_gbk_file() {
    let tmp = tempfile::tempdir().unwrap();
    let gbk: Vec<u8> = [0xD6, 0xD0, 0xCE, 0xC4, 0xD7, 0xA2, 0xCA, 0xCD].to_vec();
    let mut content = b"fn a() {}\n// ".to_vec();
    content.extend_from_slice(&gbk);
    content.extend_from_slice(b"\nfn b() {}\n".as_slice());
    std::fs::write(tmp.path().join("code.rs"), &content).unwrap();
    let m = manager_in(tmp.path());
    let out = exec_cmd(
        &m,
        "Fs_GrepFile",
        json!({"root": ".", "pattern": "中文注释"}),
    )
    .await
    .unwrap();
    assert_eq!(out["file_count"], json!(1));
    assert_eq!(out["files"][0]["encoding"], json!("gbk"));
}

#[tokio::test]
async fn find_and_grep_reject_outside_root() {
    let tmp = tempfile::tempdir().unwrap();
    let m = manager_in(tmp.path());
    let err = exec_cmd(&m, "Fs_FindFile", json!({"root": "..", "patterns": ["*"]}))
        .await
        .unwrap_err();
    assert!(matches!(err, ActError::PermissionDenied { .. }));
    let err = exec_cmd(&m, "Fs_GrepFile", json!({"root": "../..", "pattern": "x"}))
        .await
        .unwrap_err();
    assert!(matches!(err, ActError::PermissionDenied { .. }));
}
