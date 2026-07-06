<!-- BEGIN graphify-light managed codex instructions -->
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

## Token-efficient graphify-light usage

Use graphify-light only when it reduces source reads. Prefer current-session context, exact file reads, and narrow source searches for repeated questions, known files, small edits, simple overview/config/dependency/docs/build/lint/test tasks, and shell-only work.

Use graphify-light before broad scans, unclear entry points, unknown symbol ownership, call/callee tracing, import/export tracing, related-file discovery, impact analysis, and cross-file debugging:

- `find_symbol` for unknown symbols, routes, modules, functions, classes, or methods.
- `get_file_symbols` before reading a large file.
- `get_callers` / `get_callees` for execution flow.
- `get_imports` / `get_exports` for module boundaries.
- `get_related_files` before opening many files.

Before any graphify-light query, refresh the graph once for the current task (`graphify-light build` or `refresh_index`) so results match the latest code. Prefer one targeted graph query, treat results as navigation hints, verify source before editing, and fall back to direct source reads when graphify-light is insufficient.
<!-- END graphify-light managed codex instructions -->
