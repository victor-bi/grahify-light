mod detect;
mod extract;
mod graph;
mod install;
mod mcp;
mod query;

use anyhow::{bail, Result};
use clap::{Parser, Subcommand};
use graph::{read_graph, write_graph};
use install::{install_codex, InstallScope};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(name = "graphify-light")]
#[command(about = "A minimal local static code graph indexer for Codex.")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    Build,
    Query {
        #[command(subcommand)]
        command: QueryCommand,
    },
    Mcp,
    Install {
        #[command(subcommand)]
        command: InstallCommand,
    },
}

#[derive(Debug, Subcommand)]
enum QueryCommand {
    FindSymbol {
        #[arg(long)]
        name: String,
    },
    GetCallers {
        #[arg(long)]
        name: String,
    },
    GetCallees {
        #[arg(long)]
        name: String,
    },
    GetFileSymbols {
        #[arg(long)]
        path: String,
    },
    SearchNodes {
        #[arg(long)]
        text: String,
    },
    GetRelatedFiles {
        #[arg(long)]
        path: String,
    },
    GetImports {
        #[arg(long)]
        path: String,
    },
    GetExports {
        #[arg(long)]
        path: String,
    },
    GetGraphStats,
}

#[derive(Debug, Subcommand)]
enum InstallCommand {
    Codex {
        #[arg(long)]
        global: bool,
        #[arg(long)]
        project: bool,
        #[arg(long)]
        command: Option<String>,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let root = std::env::current_dir()?;

    match cli.command {
        Command::Build => {
            let graph = extract::build_graph(&root)?;
            let path = write_graph(&root, &graph)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "graph_path": path,
                    "stats": graph.stats()
                }))?
            );
        }
        Command::Query { command } => {
            let graph = read_graph(&root)?;
            let result = match command {
                QueryCommand::FindSymbol { name } => query::find_symbol(&graph, &name),
                QueryCommand::GetCallers { name } => query::get_callers(&graph, &name),
                QueryCommand::GetCallees { name } => query::get_callees(&graph, &name),
                QueryCommand::GetFileSymbols { path } => query::get_file_symbols(&graph, &path),
                QueryCommand::SearchNodes { text } => query::search_nodes(&graph, &text),
                QueryCommand::GetRelatedFiles { path } => query::get_related_files(&graph, &path),
                QueryCommand::GetImports { path } => query::get_imports(&graph, &path),
                QueryCommand::GetExports { path } => query::get_exports(&graph, &path),
                QueryCommand::GetGraphStats => query::get_graph_stats(&graph),
            };
            println!("{}", serde_json::to_string_pretty(&result)?);
        }
        Command::Mcp => {
            mcp::run_stdio(&root)?;
        }
        Command::Install { command } => match command {
            InstallCommand::Codex {
                global,
                project,
                command,
            } => {
                let scope = install_scope(global, project)?;
                let report = install_codex(&root, scope, command)?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "config_path": display_path(report.config_path),
                        "agents_path": display_path(report.agents_path),
                        "graph_path": report.graph_path.map(display_path)
                    }))?
                );
            }
        },
    }

    Ok(())
}

fn install_scope(global: bool, project: bool) -> Result<InstallScope> {
    match (global, project) {
        (true, false) => Ok(InstallScope::Global),
        (false, false) | (false, true) => Ok(InstallScope::Project),
        (true, true) => bail!("choose only one install scope: --global or --project"),
    }
}

fn display_path(path: PathBuf) -> String {
    path.to_string_lossy().replace('\\', "/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_scope_defaults_to_project() {
        assert!(matches!(
            install_scope(false, false).unwrap(),
            InstallScope::Project
        ));
    }
}
