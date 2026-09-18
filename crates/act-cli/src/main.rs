//! act: Agent Core Tools CLI + MCP stdio server.
//!
//! Two invocation styles:
//! - `act <Command> --flags ...`   (primary, schema-driven)
//! - `act exec <Command> --input '<json>'`  (JSON escape hatch)
//! plus management subcommands: list / verify / schema / package / install / mcp.

mod cli;
mod flags;
mod gen;
mod install;
mod mcp;
mod package;
mod schema;
#[cfg(test)]
mod test_support;

use act_kernel::error::ActResult;
use clap::Parser;

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

/// Restore the default SIGPIPE disposition so `act list | head` terminates
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

fn looks_like_command(token: &str) -> bool {
    act_kernel::name::CommandName::parse(token).is_ok()
        || (token.len() >= 2
            && token.len() <= 8
            && token
                .chars()
                .next()
                .map(|c| c.is_ascii_lowercase())
                .unwrap_or(false)
            && token
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit())
            && !flags::is_builtin_subcommand(token))
}

#[tokio::main]
async fn main() {
    setup_windows_console();
    setup_unix_sigpipe();
    init_tracing();

    let raw: Vec<String> = std::env::args().skip(1).collect();
    let first = raw.first().map(|s| s.as_str()).unwrap_or("");

    // Route `act <Command> --flags` to the dynamic command path when the first
    // token is a well-formed command name (Domain_Action) that is not one of
    // the builtin management subcommands.
    let dynamic = !raw.is_empty()
        && !first.starts_with('-')
        && !flags::is_builtin_subcommand(first)
        && looks_like_command(first);

    let outcome: ActResult<()> = if dynamic {
        cli::run_dynamic(first, &raw[1..]).await
    } else {
        let args = cli::Cli::parse();
        cli::run(args).await
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
