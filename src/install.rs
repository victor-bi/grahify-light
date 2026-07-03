use crate::extract::build_graph;
use crate::graph::{output_dir, write_graph};
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

#[derive(Debug)]
pub struct UninstallReport {
    pub config_path: PathBuf,
    pub agents_path: PathBuf,
    pub removed_config_block: bool,
    pub removed_agents_block: bool,
    pub purged_path: Option<PathBuf>,
}

pub fn install_codex(
    root: &Path,
    scope: InstallScope,
    command: Option<String>,
) -> Result<InstallReport> {
    let root = root
        .canonicalize()
        .with_context(|| format!("failed to resolve root {}", root.display()))?;
    let command = command.unwrap_or_else(|| "graphify-light".to_string());
    let graph_path = match scope {
        InstallScope::Project => {
            let graph = build_graph(&root)?;
            Some(write_graph(&root, &graph)?)
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
        &codex_config_block(
            &command,
            match scope {
                InstallScope::Project => Some(root.as_path()),
                InstallScope::Global => None,
            },
        ),
    )?;
    upsert_managed_block(&agents_path, AGENTS_BEGIN, AGENTS_END, &agents_block())?;

    Ok(InstallReport {
        config_path,
        agents_path,
        graph_path,
    })
}

pub fn uninstall_codex(root: &Path, scope: InstallScope, purge: bool) -> Result<UninstallReport> {
    let root = root
        .canonicalize()
        .with_context(|| format!("failed to resolve root {}", root.display()))?;
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

    let removed_config_block = remove_managed_block(&config_path, CONFIG_BEGIN, CONFIG_END)?;
    let removed_agents_block = remove_managed_block(&agents_path, AGENTS_BEGIN, AGENTS_END)?;
    let purged_path = if purge {
        let dir = output_dir(&root);
        if dir.exists() {
            fs::remove_dir_all(&dir)
                .with_context(|| format!("failed to remove {}", dir.display()))?;
        }
        Some(dir)
    } else {
        None
    };

    Ok(UninstallReport {
        config_path,
        agents_path,
        removed_config_block,
        removed_agents_block,
        purged_path,
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

pub fn remove_managed_block(path: &Path, begin: &str, end: &str) -> Result<bool> {
    let Ok(current) = fs::read_to_string(path) else {
        return Ok(false);
    };
    let Some(begin_index) = current.find(begin) else {
        return Ok(false);
    };
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
    let before = current[..begin_index].trim_end_matches(['\r', '\n']);
    let after = current[end_index..].trim_start_matches(['\r', '\n']);
    let next = match (before.is_empty(), after.is_empty()) {
        (true, true) => String::new(),
        (true, false) => format!("{after}\n"),
        (false, true) => format!("{before}\n"),
        (false, false) => format!("{before}\n\n{after}"),
    };
    fs::write(path, next).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(true)
}

fn codex_config_block(command: &str, project_root: Option<&Path>) -> String {
    let command = toml_string(command);
    let root_string = project_root.map(path_string);
    let cwd = toml_string(root_string.as_deref().unwrap_or("."));
    let args = toml_array(&["mcp"]);

    format!(
        r#"{CONFIG_BEGIN}
# This section is managed by graphify-light.
# It was added by `graphify-light install codex`.
# Do not edit inside this block manually unless you intentionally want to override graphify-light MCP behaviour.
# Existing Codex config outside this block must not be modified by graphify-light.

[mcp_servers.graphify-light]
command = {command}
args = {args}
cwd = {cwd}
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

fn toml_array(values: &[&str]) -> String {
    let values = values
        .iter()
        .map(|value| toml_string(value))
        .collect::<Vec<_>>()
        .join(", ");
    format!("[{values}]")
}

fn toml_string(value: &str) -> String {
    serde_json::to_string(value).expect("string serialization should not fail")
}

fn path_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
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

    #[test]
    fn project_config_uses_explicit_root() {
        let root = Path::new("/tmp/example repo");
        let block = codex_config_block("graphify-light", Some(root));

        assert!(block.contains(r#"command = "graphify-light""#));
        assert!(block.contains(r#"args = ["mcp"]"#));
        assert!(block.contains(r#"cwd = "/tmp/example repo""#));
        assert!(!block.contains(r#"cwd = ".""#));
    }

    #[test]
    fn managed_block_is_removed_without_touching_user_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("AGENTS.md");
        fs::write(&path, "Before\n\nBEGIN\nmanaged\nEND\n\nAfter\n").unwrap();

        let removed = remove_managed_block(&path, "BEGIN", "END").unwrap();
        let content = fs::read_to_string(&path).unwrap();

        assert!(removed);
        assert!(content.contains("Before"));
        assert!(content.contains("After"));
        assert!(!content.contains("managed"));
    }

    #[test]
    fn removing_absent_block_is_noop_success() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("AGENTS.md");
        fs::write(&path, "User text\n").unwrap();

        let removed = remove_managed_block(&path, "BEGIN", "END").unwrap();

        assert!(!removed);
        assert_eq!(fs::read_to_string(&path).unwrap(), "User text\n");
    }

    #[test]
    fn uninstall_project_removes_blocks_and_preserves_graph_without_purge() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join(".codex/config.toml");
        let agents_path = dir.path().join("AGENTS.md");
        let graph_dir = output_dir(dir.path());
        fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        fs::create_dir_all(&graph_dir).unwrap();
        fs::write(
            &config_path,
            format!("user = true\n\n{CONFIG_BEGIN}\nmanaged\n{CONFIG_END}\n"),
        )
        .unwrap();
        fs::write(
            &agents_path,
            format!("User notes\n\n{AGENTS_BEGIN}\nmanaged\n{AGENTS_END}\n"),
        )
        .unwrap();
        fs::write(graph_dir.join("graph.json"), "{}\n").unwrap();

        let report = uninstall_codex(dir.path(), InstallScope::Project, false).unwrap();

        assert!(report.removed_config_block);
        assert!(report.removed_agents_block);
        assert!(graph_dir.exists());
        assert!(fs::read_to_string(&config_path)
            .unwrap()
            .contains("user = true"));
        assert!(fs::read_to_string(&agents_path)
            .unwrap()
            .contains("User notes"));
    }

    #[test]
    fn uninstall_project_purge_removes_graph_dir() {
        let dir = tempfile::tempdir().unwrap();
        let graph_dir = output_dir(dir.path());
        fs::create_dir_all(&graph_dir).unwrap();
        fs::write(graph_dir.join("graph.json"), "{}\n").unwrap();

        let report = uninstall_codex(dir.path(), InstallScope::Project, true).unwrap();

        assert_eq!(report.purged_path, Some(graph_dir.clone()));
        assert!(!graph_dir.exists());
    }
}
