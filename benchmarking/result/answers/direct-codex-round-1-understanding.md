**Repository Understanding**

`graphify-light` is a Rust CLI that builds a deterministic local static code graph for Codex. It scans a repository, extracts code entities and relationships, writes `.ai/graphify-light/graph.json`, and exposes query/MCP tools so Codex can navigate code with less broad file reading. It intentionally excludes AI extraction, embeddings, reports, visualization, cloud sync, authentication, and non-Codex integrations.

**Architecture**

The binary is a single Rust crate with `src/main.rs` dispatching four command groups:

- `build`: scan and extract a graph, then write `.ai/graphify-light/graph.json`
- `query`: read the graph and return JSON query results
- `mcp`: run a stdio MCP server
- `install codex`: update Codex config/AGENTS managed blocks, optionally building the graph

Main modules:

- `src/detect.rs`: code file discovery using `ignore`, extension/shebang detection, language classification, size/sensitive-file filtering, skipped generated/dependency dirs.
- `src/extract.rs`: graph construction. Uses Tree-sitter for Python, JS/TS/TSX, Rust, Go, Java, C, and C++; falls back to regex heuristics for unsupported code. Adds file, definition, import, contains, import resolution, and call edges.
- `src/graph.rs`: graph data model, deterministic IDs, sorted nodes/edges, stats, path normalization, JSON read/write.
- `src/query.rs`: query helpers for symbols, callers/callees, file symbols, related files, imports, exports, and stats.
- `src/mcp.rs`: JSON-RPC stdio MCP server implementing initialize, ping, tools/list, and tools/call for graph queries plus `refresh_index`.
- `src/install.rs`: Codex integration that writes managed blocks into `.codex/config.toml` / `AGENTS.md` or global `~/.codex/*`.

**CLI / Build / Test Commands**

Primary CLI:

```bash
cargo install --path .
graphify-light build
graphify-light query find-symbol --name parse_config
graphify-light query get-callers --name parse_config
graphify-light query get-callees --name parse_config
graphify-light query get-file-symbols --path src/main.rs
graphify-light query get-related-files --path src/main.rs
graphify-light query get-imports --path src/main.rs
graphify-light query get-exports --path src/main.rs
graphify-light query get-graph-stats
graphify-light mcp
graphify-light install codex --project
graphify-light install codex --global
```

Development commands documented or implied by the Rust crate:

```bash
cargo build
cargo build --release
cargo test
cargo fmt
cargo clippy
```

Benchmarking:

```bash
docker build -f benchmarking/Dockerfile -t graphify-light-bench .
docker run --rm -e OPENAI_API_KEY -v "$PWD/benchmarking/out:/workspace/out" graphify-light-bench
```

**Generated Artifacts**

Main generated graph output:

```text
.ai/graphify-light/graph.json
```

Build outputs:

```text
target/debug/graphify-light
target/release/graphify-light
```

Codex install modifies managed blocks in:

```text
.codex/config.toml
AGENTS.md
~/.codex/config.toml
~/.codex/AGENTS.md
```

Benchmark outputs:

```text
benchmarking/out/results.md
benchmarking/out/results.json
benchmarking/out/round-*.md
benchmarking/out/logs/*.jsonl
benchmarking/out/answers/*.md
benchmarking/out/work/
```

**Integration Points**

The most important integration is Codex via MCP. `install codex` configures a local MCP server command `graphify-light mcp` and adds AGENTS guidance to prefer graph tools before broad repository scanning. MCP tools include `find_symbol`, `get_callers`, `get_callees`, `get_file_symbols`, `search_nodes`, `get_related_files`, `get_imports`, `get_exports`, `get_graph_stats`, and `refresh_index`.

The `benchmarking/` crate compares three Codex context strategies: raw repository files, upstream Graphify `graphify-out`, and graphify-light `.ai/graphify-light`. Its Dockerfile installs Codex CLI, upstream `graphifyy`, builds both Rust binaries, runs benchmark rounds, parses Codex JSONL token usage, and writes comparison reports.

Note: graphify-light MCP tools were requested by AGENTS guidance, but they were not available in this session, so this report was produced by direct read-only file inspection.