//! `act package`: stage the self-contained skill bundle and zip it.
//!
//! Layout produced under `<out>/agent-core-tools/`:
//!   SKILL.md    agent-facing command contract
//!   schema.json machine-readable contract (generated from this binary)
//!   mcp.json    MCP config template (<SKILL_DIR> placeholder)
//!   act|act.exe the kernel binary (copied from the running executable)
//! plus `<out>/agent-core-tools-<version>-<target>.zip`.

use std::io::Write;
use std::path::{Path, PathBuf};

use act_kernel::error::{ActError, ActResult};

const SKILL_MD: &str = include_str!("../../../packaging/skill/agent-core-tools/SKILL.md");
const MCP_JSON: &str = include_str!("../../../packaging/skill/agent-core-tools/mcp.json");

pub fn run(out_dir: &Path) -> ActResult<PathBuf> {
    let staging = out_dir.join("agent-core-tools");
    if staging.exists() {
        std::fs::remove_dir_all(&staging).map_err(ActError::Io)?;
    }
    std::fs::create_dir_all(&staging).map_err(ActError::Io)?;

    std::fs::write(staging.join("SKILL.md"), SKILL_MD).map_err(ActError::Io)?;
    let schema = crate::schema::build_default()?;
    let schema_pretty = serde_json::to_vec_pretty(&schema)
        .map_err(|e| ActError::Other(format!("serialize schema: {e}")))?;
    std::fs::write(staging.join("schema.json"), schema_pretty).map_err(ActError::Io)?;
    std::fs::write(staging.join("mcp.json"), MCP_JSON).map_err(ActError::Io)?;

    let exe = std::env::current_exe().map_err(ActError::Io)?;
    let exe_name = exe
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| {
            if cfg!(windows) {
                "act.exe".into()
            } else {
                "act".into()
            }
        });
    std::fs::copy(&exe, staging.join(&exe_name)).map_err(ActError::Io)?;

    let zip_name = format!(
        "agent-core-tools-{}-{}.zip",
        env!("CARGO_PKG_VERSION"),
        target_triple()
    );
    let zip_path = out_dir.join(&zip_name);
    zip_directory(&staging, &zip_path)?;

    Ok(zip_path)
}

fn target_triple() -> String {
    let arch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => other,
    };
    let os = match std::env::consts::OS {
        "windows" => "pc-windows-msvc",
        "macos" => "apple-darwin",
        "linux" => "unknown-linux-gnu",
        other => other,
    };
    format!("{arch}-{os}")
}

fn zip_directory(dir: &Path, zip_path: &Path) -> ActResult<()> {
    let file = std::fs::File::create(zip_path).map_err(ActError::Io)?;
    let mut writer = zip::ZipWriter::new(file);
    let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);

    let base = dir.parent().unwrap_or(dir);
    for entry in walkdir::WalkDir::new(dir).sort_by_file_name() {
        let entry = entry.map_err(|e| ActError::Other(format!("walk: {e}")))?;
        let path = entry.path();
        let rel = path
            .strip_prefix(base)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        if path.is_dir() {
            writer.add_directory(rel, options).map_err(zip_err)?;
        } else {
            writer.start_file(rel, options).map_err(zip_err)?;
            let bytes = std::fs::read(path).map_err(ActError::Io)?;
            writer.write_all(&bytes).map_err(ActError::Io)?;
        }
    }
    writer.finish().map_err(zip_err)?;
    Ok(())
}

fn zip_err(e: zip::result::ZipError) -> ActError {
    ActError::Other(format!("zip: {e}"))
}
