//! act: Agent Core Tools CLI + MCP stdio server.

mod cli;
mod install;
mod mcp;

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

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    // Logs to stderr only: stdout carries JSON/MCP protocol.
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .init();
}

#[tokio::main]
async fn main() {
    setup_windows_console();
    init_tracing();
    let args = cli::Cli::parse();
    match cli::run(args).await {
        Ok(()) => {}
        Err(err) => {
            eprintln!("error[{}]: {}", err.code(), err);
            std::process::exit(err.exit_code());
        }
    }
    let _ = std::io::Write::flush(&mut std::io::stdout());
    let _: ActResult<()> = Ok(());
}
