use crate::extract::build_graph;
use crate::graph::write_graph;
use anyhow::{anyhow, Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

const CONFIG_BEGIN: &str = "# BEGIN graphify-light managed codex mcp config";
const CONFIG_END: &str = "# END graphify-light managed codex mcp config";
const AGENTS_BEGIN: &str = "<!-- BEGIN graphify-light managed codex instructions -->";
const AGENTS_END: &str = "<!-- END graphify-light managed codex instructions -->";

#[derive(Debug, Clone, Copy)]
pub enum InstallScope {
    Global,
    Project,
}

#[derive(Debug)]
pub struct InstallReport {
    pub config_path: PathBuf,
    pub agents_path: PathBuf,
    pub graph_path: Option<PathBuf>,
}

pub fn install_codex(
    root: &Path,
    scope: InstallScope,
    command: Option<String>,
) -> Result<InstallReport> {
    let command = command.unwrap_or_else(|| "graphify-light".to_string());
    let graph_path = match scope {
        InstallScope::Project => {
            let graph = build_graph(root)?;
            Some(write_graph(root, &graph)?)
        }
        InstallScope::Global => None,
    };

    let (config_path, agents_path) = match scope {
        InstallScope::Project => (root.join(".codex/config.toml"), root.join("AGENTS.md")),
        InstallScope::Global => {
            let home = home_dir()?;
            (
                home.join(".codex/config.toml"),
                home.join(".codex/AGENTS.md"),
            )
        }
    };

    upsert_managed_block(
        &config_path,
        CONFIG_BEGIN,
        CONFIG_END,
        &codex_config_block(&command),
    )?;
    upsert_managed_block(&agents_path, AGENTS_BEGIN, AGENTS_END, &agents_block())?;

    Ok(InstallReport {
        config_path,
        agents_path,
        graph_path,
    })
}

pub fn upsert_managed_block(path: &Path, begin: &str, end: &str, block: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let current = fs::read_to_string(path).unwrap_or_default();
    let next = if let Some(begin_index) = current.find(begin) {
        let after_begin = begin_index + begin.len();
        let Some(relative_end_index) = current[after_begin..].find(end) else {
            return Err(anyhow!(
                "managed block in {} starts with '{}' but has no matching end marker '{}'",
                path.display(),
                begin,
                end
            ));
        };
        let end_index = after_begin + relative_end_index + end.len();
        format!(
            "{}{}{}",
            &current[..begin_index],
            block,
            &current[end_index..]
        )
    } else if current.trim().is_empty() {
        format!("{block}\n")
    } else {
        format!("{}\n\n{}\n", current.trim_end(), block)
    };
    fs::write(path, next).with_context(|| format!("failed to write {}", path.display()))
}

fn codex_config_block(command: &str) -> String {
    format!(
        r#"{CONFIG_BEGIN}
# This section is managed by graphify-light.
# It was added by `graphify-light install codex`.
# Do not edit inside this block manually unless you intentionally want to override graphify-light MCP behaviour.
# Existing Codex config outside this block must not be modified by graphify-light.

[mcp_servers.graphify-light]
command = "{command}"
args = ["mcp"]
cwd = "."
enabled = true
required = false
enabled_tools = [
  "find_symbol",
  "get_callers",
  "get_callees",
  "get_file_symbols",
  "search_nodes",
  "get_related_files",
  "get_imports",
  "get_exports",
  "get_graph_stats",
  "refresh_index"
]
default_tools_approval_mode = "auto"

{CONFIG_END}"#
    )
}

fn agents_block() -> String {
    format!(
        r#"{AGENTS_BEGIN}
<!--
This section is managed by graphify-light.
It was added by `graphify-light install codex`.
Existing AGENTS.md content outside this block must not be modified by graphify-light.
-->

## graphify-light code analysis guidance for Codex

When working in a repository that contains `.ai/graphify-light/graph.json`, use the graphify-light MCP tools before broad repository scanning for code analysis, repository navigation, symbol lookup, caller/callee tracing, import analysis, impact analysis, and related-file discovery.

Preferred workflow:

1. Use `get_graph_stats` to confirm the graph index exists and is readable.
2. Use `refresh_index` if `.ai/graphify-light/graph.json` is missing, stale, or obviously incomplete.
3. Use `find_symbol` before searching the repository for a function, class, method, module, or symbol.
4. Use `get_callers` and `get_callees` before manually tracing function calls across files.
5. Use `get_file_symbols` before reading a full source file.
6. Use `get_related_files` before opening many files.
7. Use `get_imports` and `get_exports` before manually inspecting import/export relationships.
8. Read source files only after graphify-light has identified the relevant files or symbols.

If graphify-light results are missing, stale, incomplete, or conflict with the actual source code, fall back to normal source inspection and mention that the graph index was insufficient.

Do not perform broad full-repository scans as the first step when a graphify-light MCP query can answer the structural question more directly.
{AGENTS_END}"#
    )
}

fn home_dir() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("HOME is not set; cannot install global Codex config"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn managed_block_is_appended_and_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("AGENTS.md");
        fs::write(&path, "User text\n").unwrap();

        upsert_managed_block(&path, "BEGIN", "END", "BEGIN\none\nEND").unwrap();
        let first = fs::read_to_string(&path).unwrap();
        assert!(first.contains("User text"));
        assert!(first.contains("one"));

        upsert_managed_block(&path, "BEGIN", "END", "BEGIN\ntwo\nEND").unwrap();
        let second = fs::read_to_string(&path).unwrap();
        assert!(second.contains("User text"));
        assert!(second.contains("two"));
        assert!(!second.contains("one"));
    }
}
