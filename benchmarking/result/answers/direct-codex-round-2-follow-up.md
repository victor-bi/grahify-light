The graphify-light MCP tools were not available in this session, so I inspected the raw files directly.

**CLI Dispatch**
[ src/main.rs](/workspace/out/work/direct-codex/repo/src/main.rs:14) is the entry point and command router. Key symbols:
`Cli`, `Command`, `QueryCommand`, `InstallCommand`, `main`, `install_scope`.
It dispatches:
`build` -> `extract::build_graph`
`query ...` -> `query::*`
`mcp` -> `mcp::run_stdio`
`install codex` -> `install::install_codex`.

**Graph Construction**
Graph data model and persistence live in [src/graph.rs](/workspace/out/work/direct-codex/repo/src/graph.rs:1). Key symbols:
`Graph`, `Node`, `Edge`, `GraphStats`, `GraphBuilder`, `read_graph`, `write_graph`, `graph_path`, `file_id`, `symbol_id`, `import_id`.

Actual extraction/build logic lives in [src/extract.rs](/workspace/out/work/direct-codex/repo/src/extract.rs:1). Key symbols:
`build_graph`, `extract_file`, `extract_tree_sitter`, `extract_heuristic`, `walk_tree`, `resolve_imports`, `resolve_calls`.
File discovery/language classification is in [src/detect.rs](/workspace/out/work/direct-codex/repo/src/detect.rs:1): `collect_code_files`, `DetectedFile`, `SupportedLanguage`.

**Query Handling**
[ src/query.rs](/workspace/out/work/direct-codex/repo/src/query.rs:1) implements the graph query API. Key symbols:
`find_symbol`, `get_callers`, `get_callees`, `get_file_symbols`, `search_nodes`, `get_related_files`, `get_imports`, `get_exports`, `get_graph_stats`.
CLI query dispatch is wired in `src/main.rs` through `QueryCommand`.

**MCP Handling**
[ src/mcp.rs](/workspace/out/work/direct-codex/repo/src/mcp.rs:1) is the stdio MCP server. Key symbols:
`run_stdio`, `handle_message`, `execute_tool`, `tool_definitions`, `required_string`, `success`, `error_response`, `write_message`.
It handles JSON-RPC methods including `initialize`, `ping`, `tools/list`, and `tools/call`, then maps MCP tool names to `query::*` or rebuilds via `build_graph`.

**Codex Installation Integration**
[ src/install.rs](/workspace/out/work/direct-codex/repo/src/install.rs:1) owns Codex integration. Key symbols:
`InstallScope`, `InstallReport`, `install_codex`, `upsert_managed_block`, `codex_config_block`, `agents_block`.
It writes managed blocks into `.codex/config.toml` or `~/.codex/config.toml`, plus `AGENTS.md` or `~/.codex/AGENTS.md`, registering `graphify-light mcp` and the MCP tool allowlist.