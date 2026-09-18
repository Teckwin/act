//! act: Agent Core Tools — assembly root.
//!
//! Layering (single responsibility per layer):
//! 1. `parser` (CLI): argv → `(cmd, params)` normalization ONLY
//! 2. `kernel.exec(cmd, params, ctx)`: schema conformance + path/url
//!    normalization & security blocking + dispatch + output contract + audit
//!
//! Every verb — including help and management — is a registered command.

mod cli;
mod gen;
mod install;
mod mcp;
mod package;
mod parser;
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

/// Build the runtime context from leading global flags (`--config`, `--root`)
/// and return the remaining argv for the parser. This is context assembly —
/// not command routing.
fn split_context(raw: &[String]) -> ActResult<(Option<PathBuf>, Vec<PathBuf>, Vec<String>)> {
    let mut config = None;
    let mut roots = Vec::new();
    let mut i = 0usize;
    while i < raw.len() {
        let token = &raw[i];
        let (name, inline) = match token.strip_prefix("--").and_then(|t| t.split_once('=')) {
            Some((n, v)) => (n.to_string(), Some(v.to_string())),
            None => (token.trim_start_matches('-').to_string(), None),
        };
        let value = |i: &mut usize| -> Option<String> {
            match inline.clone() {
                Some(v) => Some(v),
                None => {
                    *i += 1;
                    raw.get(*i).cloned()
                }
            }
        };
        match name.as_str() {
            "config" => {
                config = value(&mut i).map(PathBuf::from);
                if config.is_none() {
                    return Err(act_kernel::ActError::invalid_params(
                        "cli",
                        "--config expects a value",
                    ));
                }
            }
            "root" => {
                let Some(v) = value(&mut i) else {
                    return Err(act_kernel::ActError::invalid_params(
                        "cli",
                        "--root expects a value",
                    ));
                };
                roots.push(PathBuf::from(v));
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
    let outcome: ActResult<()> = match split_context(&raw) {
        Ok((config, roots, rest)) => {
            match cli::build_manager_with(config, roots) {
                Ok(manager) => {
                    let arc = std::sync::Arc::new(manager);
                    sys::bind_manager(arc.clone());
                    // Layer 1: parse (normalization). Layer 2: kernel exec.
                    let resolver = |token: &str| sys::resolve_info(token);
                    match parser::parse_command(&rest, &resolver) {
                        Ok((cmd, params)) => match arc.exec(&cmd, params).await {
                            Ok(result) => {
                                cli::print_json(&result);
                                Ok(())
                            }
                            Err(err) => Err(err),
                        },
                        Err(err) => Err(err),
                    }
                }
                Err(err) => Err(err),
            }
        }
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
