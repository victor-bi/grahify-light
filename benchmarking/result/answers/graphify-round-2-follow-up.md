Using only `graphify-out`, the architecture is represented as follows:

- **CLI dispatch:** `src/main.rs`
  - Key symbols: `Cli` at `L17`, `Command` at `L23`, `QueryCommand` at `L37`, `InstallCommand` at `L74`, `main()` at `L85`.
  - `main()` has graph links to `write_graph()` at `src/graph.rs:L232`, `read_graph()` at `src/graph.rs:L225`, `install_scope()` at `src/main.rs:L142`, and `install_codex()` at `src/install.rs:L25`.
  - `README.md` also has relevant sections: `Build A Graph`, `Query`, `Codex Integration`, and `MCP Tools`.

- **Graph construction/model:** split between `src/extract.rs` and `src/graph.rs`
  - Construction entrypoint: `build_graph()` at `src/extract.rs:L52`.
  - Build pipeline calls: `collect_code_files()` from `src/detect.rs:L66`, `extract_file()`, `resolve_imports()`, and `resolve_calls()`.
  - Graph data/model helpers: `Node`, `Edge`, `Graph`, `GraphStats`, `GraphBuilder` in `src/graph.rs:L15-L69`; serialization helpers `read_graph()` and `write_graph()` at `L225` and `L232`.

- **Query handling:** `src/query.rs`
  - Public query symbols include `find_symbol()`, `get_callers()`, `get_callees()`, `get_file_symbols()`, `search_nodes()`, `get_related_files()`, `get_imports()`, `get_exports()`, and `get_graph_stats()`.
  - Internal helpers include `symbol_nodes()`, `matching_symbol_ids()`, and `node_index()`.

- **MCP handling:** `src/mcp.rs`
  - Transport/message flow: `run_stdio()` at `L11`, `handle_message()` at `L40`, `write_message()` at `L295`.
  - Tool execution/metadata: `execute_tool()` at `L110`, `tool_definitions()` at `L184`, `tool()` at `L239`, `object_schema()` at `L248`.
  - `execute_tool()` calls `build_graph()`, `write_graph()`, and repeated `read_graph()` calls, so MCP bridges graph construction and query access.

- **Codex installation integration:** `src/install.rs`
  - Main entry: `install_codex()` at `L25`.
  - Supporting symbols: `InstallScope`, `InstallReport`, `upsert_managed_block()`, `codex_config_block()`, `agents_block()`, and `home_dir()`.
  - `install_codex()` calls `build_graph()` and `write_graph()`, then updates managed Codex/agent configuration blocks.