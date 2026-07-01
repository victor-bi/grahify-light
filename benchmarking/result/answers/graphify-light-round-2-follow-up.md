From `graph.json` only, the architecture is represented like this:

- **CLI dispatch**: `src/main.rs`
  Key graph nodes: `src_main_rs_main` (`main`, L85), `src_main_rs_cli` (`Cli`, L17), `src_main_rs_command` (`Command`, L23), `src_main_rs_querycommand` (`QueryCommand`, L37), `src_main_rs_installcommand` (`InstallCommand`, L74).
  The `main` node has call edges to graph build/write, graph read, query functions, MCP stdio, and Codex install.

- **Graph construction**: `src/extract.rs` plus `src/graph.rs`
  Entry point: `src_extract_rs_build_graph` (`build_graph`, L52).
  It calls `collect_code_files`, `extract_file`, `resolve_imports`, `resolve_calls`, and `GraphBuilder` methods via `src_graph_rs_add_node`, `src_graph_rs_add_edge`, `src_graph_rs_into_graph`.
  Data/model helpers live in `src/graph.rs`: `Graph`, `Node`, `Edge`, `GraphBuilder`, `GraphStats`, `read_graph`, `write_graph`, ID/path helpers.

- **Query handling**: `src/query.rs`
  Public query functions include `find_symbol`, `get_callers`, `get_callees`, `get_file_symbols`, `search_nodes`, `get_related_files`, `get_imports`, `get_exports`, and `get_graph_stats`.
  `src_main_rs_main` dispatches CLI query commands to these functions after `read_graph`.

- **MCP handling**: `src/mcp.rs`
  Key symbols: `run_stdio` (L11), `handle_message` (L40), `execute_tool` (L110), `tool_definitions` (L184), plus response/schema helpers `tool`, `object_schema`, `required_string`, `success`, `error_response`, `write_message`.
  `execute_tool` bridges MCP tool calls to `build_graph`, `write_graph`, `read_graph`, and the same `query.rs` functions.

- **Codex installation integration**: `src/install.rs`
  Main entry: `install_codex` (L25), with `InstallScope`, `InstallReport`, `upsert_managed_block`, `codex_config_block`, `agents_block`, and `home_dir`.
  `install_codex` calls `build_graph` and `write_graph`, then updates managed Codex config/AGENTS blocks. It is reached from CLI dispatch through `src_main_rs_main -> src_install_rs_install_codex`.