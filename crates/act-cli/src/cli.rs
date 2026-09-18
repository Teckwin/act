//! CLI definition (clap) and command dispatch.

use act_kernel::error::{ActError, ActResult};
use act_kernel::{ActConfig, CommandManager, InvokeMode};
use clap::{Parser, Subcommand};
use std::io::Read;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "act",
    version,
    about = "Agent Core Tools: sandboxed filesystem & web commands for AI agents",
    long_about = "Unified command kernel (manager + permission verifier + executor) exposing Fs_* and Web_* commands via CLI and MCP stdio server."
)]
pub struct Cli {
    /// Extra sandbox root (repeatable), added to the configured roots.
    #[arg(long, global = true)]
    pub root: Vec<PathBuf>,

    /// Explicit config file path (default: <cwd>/act.config.json or ACT_CONFIG).
    #[arg(long, global = true)]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Execute a registered command with a JSON parameter object.
    Exec {
        /// Command name, e.g. Fs_ReadFile.
        command: String,
        /// JSON parameters ('-' reads stdin).
        #[arg(long)]
        input: String,
    },
    /// List registered commands.
    List {
        /// Output raw JSON.
        #[arg(long)]
        json: bool,
    },
    /// Permission dry-run: verify without executing.
    Verify {
        command: String,
        #[arg(long)]
        input: String,
    },
    /// Run as an MCP stdio server.
    Mcp,
    /// Print the machine-readable command contract (schemas + errors + limits).
    Schema,
    /// Stage the self-contained skill bundle and zip it (SKILL.md + schema.json + mcp.json + this binary).
    Package {
        /// Output directory for the staged folder and the zip.
        #[arg(long, default_value = "dist")]
        out: PathBuf,
    },
    /// Install the skill files (SKILL.md/schema.json/mcp.json template).
    Install {
        /// Install into the current project (.claude/skills/).
        #[arg(long)]
        project: bool,
        /// Install the skill into the user directory (~/.claude/skills/).
        #[arg(long)]
        user: bool,
        /// Executable path to register in .mcp.json (default: this binary).
        #[arg(long)]
        exe: Option<String>,
        /// Also write .mcp.json registering this binary as an MCP server (project scope).
        #[arg(long)]
        with_mcp: bool,
        /// Overwrite an existing .mcp.json entry.
        #[arg(long)]
        force: bool,
    },
}

pub fn build_manager(cli: &Cli) -> ActResult<CommandManager> {
    build_manager_with(cli.config.clone(), cli.root.clone())
}

/// Build the manager from explicit config/roots (config file defaults to the
/// `ACT_CONFIG` env var or `<cwd>/act.config.json`).
pub fn build_manager_with(
    config: Option<PathBuf>,
    roots: Vec<PathBuf>,
) -> ActResult<CommandManager> {
    if let Some(path) = &config {
        let abs = if path.is_absolute() {
            path.clone()
        } else {
            std::env::current_dir().unwrap_or_default().join(path)
        };
        std::env::set_var("ACT_CONFIG", &abs);
    }
    let cwd = std::env::current_dir().map_err(ActError::Io)?;
    let mut config = ActConfig::load(&cwd)?;
    for root in &roots {
        let abs = if root.is_absolute() {
            root.clone()
        } else {
            cwd.join(root)
        };
        let canon = abs.canonicalize().map_err(|e| {
            ActError::Config(format!("--root '{}' unavailable: {}", abs.display(), e))
        })?;
        if !config.roots.contains(&canon) {
            config.roots.push(canon);
        }
    }
    let manager = CommandManager::new(config)?;
    act_fs::register_all(&manager)?;
    act_web::register_all(&manager)?;
    Ok(manager)
}

pub async fn run(cli: Cli) -> ActResult<()> {
    match cli.command {
        Command::Exec {
            ref command,
            ref input,
        } => {
            let manager = build_manager(&cli)?;
            let params = read_input(&input)?;
            let result = manager.execute(&command, params, InvokeMode::Cli).await?;
            print_json(&result);
            Ok(())
        }
        Command::List { json } => {
            let manager = build_manager(&cli)?;
            let infos = manager.list();
            if json {
                print_json(&serde_json::json!(infos));
            } else {
                println!("{:<16} {:<8} DESCRIPTION", "COMMAND", "CAP");
                for info in infos {
                    let desc: String = info.description.chars().take(90).collect();
                    println!("{:<16} {:<8} {}", info.name, info.capability, desc);
                }
            }
            Ok(())
        }
        Command::Verify {
            ref command,
            ref input,
        } => {
            let manager = build_manager(&cli)?;
            let params = read_input(&input)?;
            match manager
                .verify_only(&command, &params, InvokeMode::Cli)
                .await
            {
                Ok(verification) => {
                    print_json(&serde_json::json!({
                        "allowed": true,
                        "paths": verification.paths.iter().map(|(raw, abs, _)| serde_json::json!({
                            "input": raw,
                            "resolved": abs.to_string_lossy().replace('\\', "/"),
                        })).collect::<Vec<_>>(),
                        "urls": verification.urls.iter().map(|(raw, url)| serde_json::json!({
                            "input": raw,
                            "resolved": url.to_string(),
                        })).collect::<Vec<_>>(),
                    }));
                }
                Err(err) => {
                    print_json(&serde_json::json!({
                        "allowed": false,
                        "error": err.to_string(),
                        "code": err.code(),
                    }));
                }
            }
            Ok(())
        }
        Command::Mcp => {
            let manager = build_manager(&cli)?;
            crate::mcp::serve(manager).await
        }
        Command::Schema => {
            let manager = build_manager(&cli)?;
            print_json(&crate::schema::build(&manager));
            Ok(())
        }
        Command::Package { out } => {
            let zip_path = crate::package::run(&out)?;
            println!("staged: {}", out.join("agent-core-tools").display());
            println!("zip: {}", zip_path.display());
            Ok(())
        }
        Command::Install {
            project,
            user,
            exe,
            with_mcp,
            force,
        } => crate::install::run(project, user, exe, with_mcp, force),
    }
}

fn read_input(input: &str) -> ActResult<serde_json::Value> {
    let raw = if input == "-" {
        let mut buffer = String::new();
        std::io::stdin()
            .read_to_string(&mut buffer)
            .map_err(|e| ActError::Other(format!("read stdin: {e}")))?;
        buffer
    } else {
        input.to_string()
    };
    serde_json::from_str(&raw)
        .map_err(|e| ActError::invalid_params("input", format!("invalid JSON input: {e}")))
}

pub fn print_json(value: &serde_json::Value) {
    match serde_json::to_string_pretty(value) {
        Ok(text) => println!("{text}"),
        Err(e) => eprintln!("serialize output failed: {e}"),
    }
}

/// Execute a dynamic command: `act <Command> --flags ...`.
/// The first token is a registered command name (Domain_Action); the rest are
/// schema-driven flags parsed by `flags::parse`.
pub async fn run_dynamic(name: &str, args: &[String]) -> ActResult<()> {
    let manager = build_manager_with(None, Vec::new())?;
    // Canonical command name or short alias (fr → Fs_ReadFile).
    let def = manager
        .resolve_def(name)
        .ok_or_else(|| ActError::UnknownCommand(name.to_string()))?;
    let canonical = def.name.as_str().to_string();
    let infos = manager.list();
    let info = infos
        .iter()
        .find(|c| c.name == canonical)
        .ok_or_else(|| ActError::UnknownCommand(canonical.clone()))?;
    let params = crate::flags::parse(info, args)?;
    let result = manager.execute(&canonical, params, InvokeMode::Cli).await?;
    print_json(&result);
    Ok(())
}
