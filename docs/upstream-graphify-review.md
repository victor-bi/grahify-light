# Upstream Graphify Review

This document records the upstream Graphify review that shaped `graphify-light`.

Reviewed upstream repository:

- `https://github.com/safishamsi/graphify`
- `ARCHITECTURE.md`
- `graphify/detect.py`
- `graphify/extract.py`
- `graphify/build.py`
- `graphify/export.py`
- `graphify/serve.py`

## Relevant Upstream Model

Upstream Graphify documents this core pipeline:

```text
detect() -> extract() -> build_graph() -> cluster() -> analyze() -> report() -> export()
```

For `graphify-light`, only the deterministic code graph subset is retained:

```text
detect code files -> extract nodes and edges -> build graph data -> export graph.json -> query / MCP
```

The upstream extraction schema is conceptually:

```json
{
  "nodes": [
    {
      "id": "unique_string",
      "label": "human name",
      "source_file": "path",
      "source_location": "L42"
    }
  ],
  "edges": [
    {
      "source": "id_a",
      "target": "id_b",
      "relation": "calls|imports|uses|...",
      "confidence": "EXTRACTED|INFERRED|AMBIGUOUS"
    }
  ]
}
```

`graphify-light` preserves the same core `nodes` / `edges` shape, source provenance fields, relationship naming style, and deterministic confidence labels.

## Detection

Upstream Graphify has broad code, document, paper, image, office, and video detection. `graphify-light` only keeps code detection.

`graphify-light`:

- Scans the current repository.
- Honors `.gitignore` via the `ignore` crate.
- Skips generated or dependency-heavy directories such as `.git`, `target`, `node_modules`, `.ai/graphify-light`, and build outputs.
- Sorts files before extraction.
- Uses repo-relative paths in graph output.

## Extraction

Upstream Graphify uses Tree-sitter based deterministic extraction for supported source languages, then performs cross-file resolution for imports and calls.

`graphify-light` follows the same conceptual path:

```text
source file
-> language detection
-> Tree-sitter parser selection where supported
-> AST traversal
-> file/class/function/method/import/call extraction
-> cross-file import and call resolution
-> graph JSON export
```

The initial Rust implementation supports Tree-sitter extraction for:

- Python
- JavaScript
- TypeScript
- TSX
- Rust
- Go
- Java
- C
- C++

Unsupported source extensions use a conservative heuristic fallback. The fallback is intentionally limited to file nodes, simple definitions, imports, and obvious call syntax. This is a documented deviation from upstream and exists only so unsupported languages still produce useful structural anchors instead of an empty graph.

## Build And Export

Upstream Graphify builds a NetworkX graph and exports node-link JSON, restoring directional endpoints for edges such as `calls`.

`graphify-light` does not use NetworkX because it is implemented in Rust. Instead, it keeps a typed in-memory graph, deduplicates nodes by ID, deduplicates edges by `(source, target, relation, source_location)`, prunes dangling edges where appropriate, and writes sorted JSON.

Intentional export differences:

- The output path is `.ai/graphify-light/graph.json` instead of `graphify-out/graph.json`.
- No `graph.html`, `GRAPH_REPORT.md`, SVG, GraphML, Obsidian, Neo4j, or visualization artifacts are generated.
- No clustering, Leiden communities, semantic report, LLM extraction, or embedding fields are generated.
- Node IDs include sanitized repo-relative file paths with extensions to avoid collisions. This is close to Graphify's stable path-based ID strategy but not byte-identical to every Graphify version.

## Confidence Labels

`graphify-light` keeps Graphify-style labels:

- `EXTRACTED`: Directly present in source syntax, such as definitions, imports, same-file calls, and import-backed cross-file calls.
- `INFERRED`: Deterministic second-pass inference, such as a cross-file call matched by a unique symbol name without direct import evidence.
- `AMBIGUOUS`: Reserved for future deterministic extraction cases where the graph can keep an uncertain relationship without pretending it is exact.

No AI, LLM, or embedding process contributes to confidence labels.

## MCP

Upstream Graphify exposes query behavior through an MCP server. `graphify-light` implements only a Codex-focused stdio MCP mode with lower_snake_case tool names and small structured query responses.

The MCP implementation follows the 2025-06-18 MCP lifecycle and tool methods:

- `initialize`
- `notifications/initialized`
- `ping`
- `tools/list`
- `tools/call`

## Non-Goals

`graphify-light` intentionally excludes:

- AI extraction
- LLM calls
- embeddings
- vector databases
- reports
- visualization
- document/PDF/image/video parsing
- cloud sync
- authentication
- web UI
- assistant integrations other than Codex
