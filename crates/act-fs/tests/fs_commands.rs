//! Integration tests: security matrix, encoding roundtrips, trash semantics.

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

async fn exec(m: &CommandManager, params: Value) -> Result<Value, ActError> {
    m.execute("Fs_ReadFile", params, InvokeMode::Cli).await
}

async fn exec_cmd(m: &CommandManager, command: &str, params: Value) -> Result<Value, ActError> {
    m.execute(command, params, InvokeMode::Cli).await
}

fn first_error(result: &Value) -> String {
    result["results"][0]["error"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}

// ---------- Security matrix ----------

#[tokio::test]
async fn read_traversal_denied() {
    let tmp = tempfile::tempdir().unwrap();
    let m = manager_in(tmp.path());
    for bad in ["../../x", "a/../../..", "/etc/passwd"] {
        let err = exec(&m, json!({"paths": [bad]})).await.unwrap_err();
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
        let err = exec_cmd(
            &m,
            "Fs_WriteFile",
            json!({"files": [{"path": bad, "content": "x"}]}),
        )
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
        ("Fs_ReadFile", json!({"paths": [".git/config"]})),
        ("Fs_ReadFile", json!({"paths": [".env"]})),
        (
            "Fs_WriteFile",
            json!({"files": [{"path": ".git/config", "content": "x"}]}),
        ),
        (
            "Fs_AppendFile",
            json!({"files": [{"path": ".env", "content": "x"}]}),
        ),
        ("Fs_RemoveFile", json!({"paths": [".env"]})),
        (
            "Fs_RemoveDir",
            json!({"paths": [".git"], "recursive": true}),
        ),
        (
            "Fs_MoveFile",
            json!({"moves": [{"from": ".env", "to": "leak.txt"}]}),
        ),
        (
            "Fs_EditFile",
            json!({"files": [{"path": ".env", "edits": [{"old": "S", "new": "X"}]}]}),
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
    let out = exec_cmd(
        &m,
        "Fs_RemoveDir",
        json!({"paths": ["."], "recursive": true}),
    )
    .await
    .unwrap();
    assert_eq!(out["ok"], json!(false));
    assert!(
        first_error(&out).contains("sandbox root"),
        "got: {}",
        first_error(&out)
    );
    assert!(tmp.path().is_dir(), "root itself must survive");
}

#[tokio::test]
async fn non_recursive_remove_dir_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("d")).unwrap();
    std::fs::write(tmp.path().join("d/f.txt"), b"x").unwrap();
    let m = manager_in(tmp.path());
    let out = exec_cmd(&m, "Fs_RemoveDir", json!({"paths": ["d"]}))
        .await
        .unwrap();
    assert_eq!(out["ok"], json!(false));
    assert!(first_error(&out).contains("recursive"));
    assert!(tmp.path().join("d/f.txt").exists());
}

// ---------- Trash ----------

#[tokio::test]
async fn remove_file_goes_to_trash() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("t.txt"), b"data").unwrap();
    let m = manager_in(tmp.path());
    let out = exec_cmd(&m, "Fs_RemoveFile", json!({"paths": ["t.txt"]}))
        .await
        .unwrap();
    assert_eq!(out["ok"], json!(true));
    assert_eq!(out["results"][0]["mode"], json!("trash"));
    let trash = out["results"][0]["trash_path"].as_str().unwrap();
    assert!(trash.starts_with(".act/trash/"), "trash path: {trash}");
    assert!(tmp.path().join(trash).is_file(), "trashed file must exist");
    assert!(!tmp.path().join("t.txt").exists());
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
    let out = exec_cmd(&m, "Fs_RemoveFile", json!({"paths": ["p.txt"]}))
        .await
        .unwrap();
    assert_eq!(out["results"][0]["mode"], json!("permanent"));
    assert!(!tmp.path().join("p.txt").exists());
}

#[tokio::test]
async fn trash_entry_can_be_read_back() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("rb.txt"), "recoverable").unwrap();
    let m = manager_in(tmp.path());
    let out = exec_cmd(&m, "Fs_RemoveFile", json!({"paths": ["rb.txt"]}))
        .await
        .unwrap();
    let trash = out["results"][0]["trash_path"]
        .as_str()
        .unwrap()
        .to_string();
    // .act/** is protected, but overflow/trash entries stay readable.
    let out = exec(&m, json!({"paths": [trash]})).await.unwrap();
    assert_eq!(out["results"][0]["ok"], json!(true));
    assert_eq!(out["results"][0]["content"], json!("recoverable"));
}

// ---------- Encoding ----------

#[tokio::test]
async fn read_gbk_file() {
    let tmp = tempfile::tempdir().unwrap();
    // "你好，世界" encoded in GBK.
    let gbk_bytes: Vec<u8> = [0xC4, 0xE3, 0xBA, 0xC3, 0xA3, 0xAC, 0xCA, 0xC0, 0xBD, 0xE7].to_vec();
    std::fs::write(tmp.path().join("cn.txt"), &gbk_bytes).unwrap();
    let m = manager_in(tmp.path());
    let out = exec(&m, json!({"paths": ["cn.txt"]})).await.unwrap();
    assert_eq!(out["results"][0]["encoding"], json!("gbk"));
    assert_eq!(out["results"][0]["content"], json!("你好，世界"));
}

#[tokio::test]
async fn write_read_roundtrip_gbk() {
    let tmp = tempfile::tempdir().unwrap();
    let m = manager_in(tmp.path());
    let out = exec_cmd(
        &m,
        "Fs_WriteFile",
        json!({"files": [{"path": "gb.txt", "content": "中文内容测试", "encoding": "gbk"}]}),
    )
    .await
    .unwrap();
    assert_eq!(out["ok"], json!(true));
    let raw = std::fs::read(tmp.path().join("gb.txt")).unwrap();
    assert_eq!(
        raw,
        vec![0xD6, 0xD0, 0xCE, 0xC4, 0xC4, 0xDA, 0xC8, 0xDD, 0xB2, 0xE2, 0xCA, 0xD4]
    );

    let out = exec(&m, json!({"paths": ["gb.txt"]})).await.unwrap();
    assert_eq!(out["results"][0]["encoding"], json!("gbk"));
    assert_eq!(out["results"][0]["content"], json!("中文内容测试"));
}

#[tokio::test]
async fn edit_preserves_gbk_encoding() {
    let tmp = tempfile::tempdir().unwrap();
    let m = manager_in(tmp.path());
    exec_cmd(
        &m,
        "Fs_WriteFile",
        json!({"files": [{"path": "e.txt", "content": "旧的内容", "encoding": "gbk"}]}),
    )
    .await
    .unwrap();
    let out = exec_cmd(
        &m,
        "Fs_EditFile",
        json!({"files": [{"path": "e.txt", "edits": [{"old": "旧", "new": "新"}]}]}),
    )
    .await
    .unwrap();
    assert_eq!(out["ok"], json!(true));
    assert_eq!(out["results"][0]["encoding"], json!("gbk"));
    let out = exec(&m, json!({"paths": ["e.txt"]})).await.unwrap();
    assert_eq!(out["results"][0]["content"], json!("新的内容"));
}

#[tokio::test]
async fn utf16_bom_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let m = manager_in(tmp.path());
    exec_cmd(
        &m,
        "Fs_WriteFile",
        json!({"files": [{"path": "u16.txt", "content": "hello 世界", "encoding": "utf-16le"}]}),
    )
    .await
    .unwrap();
    let out = exec(&m, json!({"paths": ["u16.txt"]})).await.unwrap();
    assert_eq!(out["results"][0]["encoding"], json!("utf-16le"));
    assert_eq!(out["results"][0]["content"], json!("hello 世界"));
}

// ---------- Batch semantics ----------

#[tokio::test]
async fn batch_partial_failure_isolated() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("a.txt"), b"a").unwrap();
    let m = manager_in(tmp.path());
    // Runtime failures (missing file) are isolated per item.
    let out = exec(&m, json!({"paths": ["a.txt", "missing.txt"]}))
        .await
        .unwrap();
    assert_eq!(out["summary"]["succeeded"], json!(1));
    assert_eq!(out["summary"]["failed"], json!(1));
    assert_eq!(out["results"][0]["ok"], json!(true));
    assert_eq!(out["results"][1]["ok"], json!(false));

    // Security failures (escape) reject the whole call pre-execution.
    let err = exec(&m, json!({"paths": ["a.txt", "../escape.txt"]}))
        .await
        .unwrap_err();
    assert!(matches!(err, ActError::PermissionDenied { .. }));
}

#[tokio::test]
async fn line_slicing() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("lines.txt"), "l1\nl2\nl3\nl4\nl5\n").unwrap();
    let m = manager_in(tmp.path());
    let out = exec(&m, json!({"paths": ["lines.txt"], "offset": 1, "limit": 2}))
        .await
        .unwrap();
    assert_eq!(out["results"][0]["content"], json!("l2\nl3"));
    assert_eq!(out["results"][0]["total_lines"], json!(5));
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
    assert!(paths.iter().any(|p| p.contains("main.rs")));
}

#[tokio::test]
async fn grep_chinese_content_in_gbk_file() {
    let tmp = tempfile::tempdir().unwrap();
    // GBK "中文注释" + code
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
    assert_eq!(out["files"][0]["match_count"], json!(1));
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

// ---------- Move / Copy / ListDir / CreateDir ----------

#[tokio::test]
async fn move_and_copy_dirs() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("src/a")).unwrap();
    std::fs::write(tmp.path().join("src/a/f.txt"), b"x").unwrap();
    let m = manager_in(tmp.path());

    let out = exec_cmd(
        &m,
        "Fs_MoveDir",
        json!({"moves": [{"from": "src", "to": "renamed"}]}),
    )
    .await
    .unwrap();
    assert_eq!(out["ok"], json!(true));
    assert!(tmp.path().join("renamed/a/f.txt").is_file());
    assert!(!tmp.path().join("src").exists());

    let out = exec_cmd(
        &m,
        "Fs_CopyDir",
        json!({"moves": [{"from": "renamed", "to": "copy"}]}),
    )
    .await
    .unwrap();
    assert_eq!(out["ok"], json!(true));
    assert!(tmp.path().join("copy/a/f.txt").is_file());

    // Destination exists without overwrite -> rejected.
    let out = exec_cmd(
        &m,
        "Fs_CopyDir",
        json!({"moves": [{"from": "copy", "to": "renamed"}]}),
    )
    .await
    .unwrap();
    assert_eq!(out["ok"], json!(false));
}

#[tokio::test]
async fn move_into_own_child_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(tmp.path().join("parent/child")).unwrap();
    let m = manager_in(tmp.path());
    let out = exec_cmd(
        &m,
        "Fs_MoveDir",
        json!({"moves": [{"from": "parent", "to": "parent/child/sub"}]}),
    )
    .await
    .unwrap();
    assert_eq!(out["ok"], json!(false));
}

#[tokio::test]
async fn list_dir_and_create_dir() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("x.txt"), b"1").unwrap();
    std::fs::create_dir_all(tmp.path().join("sub")).unwrap();
    let m = manager_in(tmp.path());

    let out = exec_cmd(&m, "Fs_ListDir", json!({"paths": ["."], "depth": 2}))
        .await
        .unwrap();
    let entries = out["results"][0]["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 2); // x.txt + sub (sub has no children)
    let _ = entries;

    let out = exec_cmd(&m, "Fs_CreateDir", json!({"paths": ["a/b/c"]}))
        .await
        .unwrap();
    assert_eq!(out["results"][0]["created"], json!(true));
    assert!(tmp.path().join("a/b/c").is_dir());
}

#[tokio::test]
async fn file_info_reports_metadata() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("i.txt"), b"12345").unwrap();
    let m = manager_in(tmp.path());
    let out = exec_cmd(&m, "Fs_FileInfo", json!({"paths": ["i.txt"]}))
        .await
        .unwrap();
    assert_eq!(out["results"][0]["size"], json!(5));
    assert_eq!(out["results"][0]["is_file"], json!(true));
    assert!(out["results"][0]["modified"].as_str().is_some());
}

#[tokio::test]
async fn edit_ambiguity_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::write(tmp.path().join("amb.txt"), "a a a").unwrap();
    let m = manager_in(tmp.path());
    let out = exec_cmd(
        &m,
        "Fs_EditFile",
        json!({"files": [{"path": "amb.txt", "edits": [{"old": "a", "new": "b"}]}]}),
    )
    .await
    .unwrap();
    assert_eq!(out["ok"], json!(false));
    assert!(first_error(&out).contains("3 times"));
    // File untouched.
    assert_eq!(
        std::fs::read_to_string(tmp.path().join("amb.txt")).unwrap(),
        "a a a"
    );
}
