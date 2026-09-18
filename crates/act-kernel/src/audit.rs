//! Append-only JSONL audit log: every command invocation is recorded.

use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Debug, Serialize)]
pub struct AuditEntry {
    pub ts: DateTime<Utc>,
    pub mode: String,
    pub command: String,
    pub paths: Vec<String>,
    pub urls: Vec<String>,
    pub allowed: bool,
    pub error_code: Option<String>,
    pub error: Option<String>,
    pub duration_ms: u64,
}

pub struct AuditLog {
    path: PathBuf,
    lock: Mutex<()>,
}

impl AuditLog {
    /// Create the audit log (and parent directories) under the given path.
    pub fn new(path: PathBuf) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        Ok(Self {
            path,
            lock: Mutex::new(()),
        })
    }

    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    pub fn record(&self, entry: AuditEntry) {
        let line = match serde_json::to_string(&entry) {
            Ok(l) => l,
            Err(e) => {
                tracing::error!("audit serialize failed: {e}");
                return;
            }
        };
        let _guard = self.lock.lock();
        let result = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
            .and_then(|mut file| {
                file.write_all(line.as_bytes())?;
                file.write_all(b"\n")
            });
        if let Err(e) = result {
            tracing::error!("audit write failed at {}: {e}", self.path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_entries_as_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let log = AuditLog::new(dir.path().join(".act/audit.jsonl")).unwrap();
        log.record(AuditEntry {
            ts: Utc::now(),
            mode: "cli".into(),
            command: "Fs_ReadFile".into(),
            paths: vec!["a.txt".into()],
            urls: vec![],
            allowed: true,
            error_code: None,
            error: None,
            duration_ms: 5,
        });
        log.record(AuditEntry {
            ts: Utc::now(),
            mode: "mcp".into(),
            command: "Web_Fetch".into(),
            paths: vec![],
            urls: vec!["https://example.com".into()],
            allowed: false,
            error_code: Some("permission_denied".into()),
            error: Some("blocked".into()),
            duration_ms: 1,
        });
        let raw = std::fs::read_to_string(dir.path().join(".act/audit.jsonl")).unwrap();
        let lines: Vec<&str> = raw.lines().collect();
        assert_eq!(lines.len(), 2);
        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["command"], "Fs_ReadFile");
        assert_eq!(first["allowed"], true);
    }
}
