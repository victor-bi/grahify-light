**Repository Understanding Report**

Purpose: `graphify-light` is a Rust code-analysis tool that builds a lightweight repository graph, then exposes graph queries for Codex/MCP workflows. The graph model tracks files, symbols, imports, calls, references, and document sections.

Architecture: the core flow is `detect -> extract -> graph -> query`, with `src/main.rs` as the CLI entry point and `src/mcp.rs` as an MCP stdio server. The graph has 258 nodes and 755 links in this Graphify output, mostly `references`, `contains`, `calls`, and `imports_from`.

Main modules:
- `src/detect.rs`: file discovery and filtering. Key functions include `collect_code_files`, `is_code_file`, `classify_language`, sensitivity/size filters, and shebang detection.
- `src/extract.rs`: graph extraction. It uses tree-sitter when possible, falls back to heuristics, records imports/definitions/calls, resolves local imports/calls, and skips test-like paths.
- `src/graph.rs`: graph data model and serialization helpers. Defines `Node`, `Edge`, `Graph`, `GraphStats`, `GraphBuilder`, path normalization, ID generation, sorting, `read_graph`, and `write_graph`.
- `src/query.rs`: read-only graph query API: symbol lookup, callers/callees, file symbols, related files, imports, exports, node search, and stats.
- `src/main.rs`: CLI parser/dispatcher. Graph nodes show `Command`, `QueryCommand`, and `InstallCommand` enums plus `main`, `install_scope`, and display helpers.
- `src/mcp.rs`: JSON/MCP stdio integration. It defines tool schemas, request handling, graph build/read/write execution, success/error responses, and message writing.
- `src/install.rs`: Codex installation support. It generates managed config/AGENTS blocks and calls `build_graph` plus `write_graph`.
- `benchmarking/src/main.rs`: benchmarking harness comparing variants, running Codex commands, parsing token usage, generating markdown reports and round tables.

CLI/build/test commands: the exact command text is not fully preserved in `graph.json`; the README headings indicate Usage sections for Install, Build A Graph, Query, Codex Integration, MCP Tools, and Development. As a Rust project using `clap`, likely commands are exposed through the main binary’s `Command`, `QueryCommand`, and `InstallCommand`. Build/test commands should be standard Cargo workflows such as `cargo build`, `cargo build --release`, and `cargo test`, but the precise documented forms are not recoverable from the Graphify artifacts alone.

Generated artifacts in this directory:
- `graph.json`: merged repository graph.
- `manifest.json`: analyzed file manifest with AST hashes.
- `cache/stat-index.json`: cached per-file stats.
- `cache/ast/v0.9.3/*.json`: per-file extracted AST graph fragments.

Notable integration points:
- Tree-sitter extraction via `tree_sitter` and language classification.
- JSON serialization via `serde`/`serde_json`.
- CLI parsing via `clap`.
- Codex integration through `install_codex`, managed `AGENTS.md`/config blocks, and MCP stdio tools.
- Benchmarking invokes Codex-like command runs, captures token usage, and emits markdown/table reports.