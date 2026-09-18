//! act: Agent Core Tools — single dynamic CLI.
//!
//! One invocation style: `act <Command|alias> --flags ...`. All verbs,
//! including the kernel management commands (Sys_List/Sys_Verify/Sys_Schema/
//! Sys_Package/Sys_Install/Sys_Serve), are registered through the same
//! CommandBuilder contract pipeline. Global flags (`--config`, `--root`) may
//! precede the command name.

mod cli;
mod flags;
mod gen;
mod install;
mod mcp;
mod package;
mod schema;
mod sys;
#[cfg(test)]
mod test_support;

use act_kernel::error::ActResult;
use std::path::PathBuf;

fn setup_windows_console() {
    #[cfg(windows)]
    {
        // UTF-8 console: the definitive fix for mojibake on Windows agents.
        unsafe {
            windows_sys::Win32::System::Console::SetConsoleOutputCP(65001);
            windows_sys::Win32::System::Console::SetConsoleCP(65001);
        }
    }
}

/// Restore the default SIGPIPE disposition so `act Sys_List | head` terminates
/// quietly like cat/ls instead of panicking on a broken stdout pipe.
fn setup_unix_sigpipe() {
    #[cfg(unix)]
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    // Logs to stderr only: stdout carries JSON output.
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .init();
}

const HELP: &str = "\
act <Command|alias> --flags ...        # only invocation style

Global flags (before the command name):
  --config <path>   explicit config file (default: <cwd>/act.config.json or ACT_CONFIG)
  --root <path>     extra sandbox root (repeatable)

Discover commands:
  act Sys_List                        # (alias: list) all commands + aliases
  act Sys_Schema                      # (alias: schema) full machine contract
  act Sys_Verify --target fr --path a # (alias: verify) permission dry-run

Examples:
  act Fs_ReadFile --path src/main.rs --limit 100
  act fr -p src/main.rs               # short aliases
  act Fs_EditFile --path a.rs --edit \"old=>new\"
";

/// Split leading global flags (`--config v`, `--root v`, repeatable) from the
/// rest of the command line.
fn split_globals(raw: &[String]) -> ActResult<(Option<PathBuf>, Vec<PathBuf>, Vec<String>)> {
    let mut config = None;
    let mut roots = Vec::new();
    let mut i = 0usize;
    while i < raw.len() {
        let token = &raw[i];
        let (name, inline) = match token.strip_prefix("--").and_then(|t| t.split_once('=')) {
            Some((n, v)) => (n.to_string(), Some(v.to_string())),
            None => (token.trim_start_matches('-').to_string(), None),
        };
        match name.as_str() {
            "config" => {
                let value = match inline {
                    Some(v) => v,
                    None => {
                        i += 1;
                        match raw.get(i) {
                            Some(v) => v.clone(),
                            None => {
                                return Err(act_kernel::ActError::invalid_params(
                                    "cli",
                                    "--config expects a value",
                                ));
                            }
                        }
                    }
                };
                config = Some(PathBuf::from(value));
            }
            "root" => {
                let value = match inline {
                    Some(v) => v,
                    None => {
                        i += 1;
                        match raw.get(i) {
                            Some(v) => v.clone(),
                            None => {
                                return Err(act_kernel::ActError::invalid_params(
                                    "cli",
                                    "--root expects a value",
                                ));
                            }
                        }
                    }
                };
                roots.push(PathBuf::from(value));
            }
            _ => break,
        }
        i += 1;
    }
    Ok((config, roots, raw[i..].to_vec()))
}

#[tokio::main]
async fn main() {
    setup_windows_console();
    setup_unix_sigpipe();
    init_tracing();

    let raw: Vec<String> = std::env::args().skip(1).collect();
    if raw.is_empty() || raw.iter().any(|t| t == "-h" || t == "--help") {
        print!("{HELP}");
        return;
    }
    if raw.iter().any(|t| t == "-V" || t == "--version") {
        println!("act {}", env!("CARGO_PKG_VERSION"));
        return;
    }

    let outcome: ActResult<()> = match split_globals(&raw) {
        Ok((_config, _roots, rest)) if rest.is_empty() => {
            print!("{HELP}");
            Ok(())
        }
        Ok((config, roots, rest)) => match cli::build_manager_with(config, roots) {
            Ok(manager) => {
                crate::sys::bind_manager(std::sync::Arc::new(manager));
                cli::run_dynamic(&rest[0], &rest[1..]).await
            }
            Err(err) => Err(err),
        },
        Err(err) => Err(err),
    };

    match outcome {
        Ok(()) => {}
        Err(err) => {
            eprintln!("error[{}]: {}", err.code(), err);
            std::process::exit(err.exit_code());
        }
    }
    let _ = std::io::Write::flush(&mut std::io::stdout());
}
