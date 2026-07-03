# graphify-light

`graphify-light` is a small, local, deterministic code graph indexer for Codex.

It keeps the useful core of Graphify's code graph workflow:

```text
repo -> deterministic static code analysis -> .ai/graphify-light/graph.json -> query / MCP for Codex
```

It intentionally does not implement AI extraction, embeddings, vector search, reports, HTML visualization, cloud sync, authentication, or non-Codex assistant integrations.

## Usage

### Install

Build and install the Rust binary somewhere on `PATH`:

```bash
cargo install --path .
```

Codex can also launch it by absolute path if you edit the generated managed MCP config block.

### Build A Graph

```bash
graphify-light build
```

The graph is written to:

```text
.ai/graphify-light/graph.json
```

The output is deterministic for the same repository state: files are processed in sorted order, paths are repo-relative, nodes and edges are sorted before write, and no timestamp is embedded in structural graph content.

### Query

```bash
graphify-light query find-symbol --name parse_config
graphify-light query get-callers --name parse_config
graphify-light query get-callees --name parse_config
graphify-light query get-file-symbols --path src/main.rs
graphify-light query search-nodes --text config
graphify-light query get-related-files --path src/main.rs
graphify-light query get-imports --path src/main.rs
graphify-light query get-exports --path src/main.rs
graphify-light query get-graph-stats
```

Query output is JSON.

### Codex Integration

Project install:

```bash
graphify-light install codex
```

This rebuilds `.ai/graphify-light/graph.json`, then updates:

```text
.codex/config.toml
AGENTS.md
```

The project MCP config pins the server to the absolute repository path used
at install time, so Codex reads this repository's graph even if the MCP
process is launched from another working directory.

Use `--project` if you prefer to make the default scope explicit.

Global install:

```bash
graphify-light install codex --global
```

This updates:

```text
~/.codex/config.toml
~/.codex/AGENTS.md
```

Both commands use managed blocks and leave user content outside those blocks untouched.

This is best-effort Codex integration. It exposes `graphify-light mcp` as a local stdio MCP server and gives Codex guidance to prefer graph queries before broad repository scanning, but it does not replace Codex's native file-reading or code-search behavior.

### MCP Tools

`graphify-light mcp` exposes these lower_snake_case tools:

```text
find_symbol
get_callers
get_callees
get_file_symbols
search_nodes
get_related_files
get_imports
get_exports
get_graph_stats
refresh_index
```

## Development

### Toolchain

Install the latest stable Rust toolchain with rustup:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
. "$HOME/.cargo/env"
rustup default stable
rustup update stable
```

Verify that the rustup-managed tools are first on `PATH`:

```bash
command -v cargo
cargo --version
rustc --version
```

### Build From Source

Build a release binary:

```bash
cargo build --release
```

The binary is written to:

```text
target/release/graphify-light
```

For local development, a debug build is enough:

```bash
cargo build
```

The debug binary is written to:

```text
target/debug/graphify-light
```

### Install The Release Binary Locally

Use this script to build the release binary and install it into the current user's global bin directory:

```bash
#!/usr/bin/env bash
set -euo pipefail

cargo build --release

BIN_DIR="$HOME/.local/bin"
mkdir -p "$BIN_DIR"
install -m 755 target/release/graphify-light "$BIN_DIR/graphify-light"

echo "Installed graphify-light to $BIN_DIR/graphify-light"
echo "Make sure $BIN_DIR is on PATH."
```

For a machine-wide install to `/usr/local/bin`, replace the `install` line with:

```bash
sudo install -m 755 target/release/graphify-light /usr/local/bin/graphify-light
```

## Graph Model

The graph file has the core shape:

```json
{
  "nodes": [],
  "edges": []
}
```

Nodes represent code entities such as files, modules, classes, functions, methods, imports, and symbols. Edges represent relationships such as `contains`, `imports`, `imports_from`, `calls`, `references`, `defines`, and `inherits`.

Edge confidence labels are deterministic:

```text
EXTRACTED
INFERRED
AMBIGUOUS
```

`graphify-light` does not use AI-generated confidence.

## Upstream Review

Before implementation, the upstream Graphify code graph path was reviewed. See [docs/upstream-graphify-review.md](docs/upstream-graphify-review.md) for compatibility notes and intentional deviations.

## Benchmarking

**Simple Benchmarking Conclusion**

Graphify and graphify-light differ mainly in how much context they prepare for Codex. Graphify gives Codex a larger and more complete generated repository context. graphify-light gives Codex a smaller, more direct code graph. In the first round, where Codex was asked to understand the whole repository, graphify-light used the fewest tokens because it gave Codex much less content to read than raw files or Graphify.

In the second round, where Codex answered a narrower follow-up question against the same prepared context, Direct Codex used the fewest tokens. That suggests Codex's native context and cache can already be efficient for follow-up questions, and extra graph context does not always keep saving tokens. Graphify performed much worse in that round because its prepared context was too heavy. graphify-light also used slightly more tokens than Direct Codex in the second round, but the gap was much smaller.

Across both rounds combined, graphify-light used the fewest total tokens, Direct Codex came second, and Graphify used the most. The practical conclusion is not that graph context is always cheaper. The conclusion is that graphify-light helps most on first-pass repository understanding and is cheaper overall in this benchmark, while native Codex can still be more efficient for narrow follow-up questions.

This repository includes a containerized Rust benchmark under [`benchmarking/`](benchmarking/). It follows upstream Graphify's token benchmark shape: run the same repository-understanding task against raw files, Graphify's generated context, and graphify-light's generated graph, then compare Codex token usage.

Build the benchmark image from the repository root:

```bash
docker build -f benchmarking/Dockerfile -t graphify-light-bench .
```

Run it with an OpenAI API key:

```bash
mkdir -p benchmarking/result
docker run --rm \
  -u "$(id -u):$(id -g)" \
  -e OPENAI_API_KEY \
  -e HOME=/tmp \
  -e CODEX_HOME=/tmp/codex \
  -v "$PWD/benchmarking/result:/workspace/out" \
  graphify-light-bench
```

Or run it with an existing Codex login:

```bash
mkdir -p benchmarking/result
docker run --rm \
  -u "$(id -u):$(id -g)" \
  -e HOME=/tmp \
  -e CODEX_HOME=/tmp/codex \
  -v "$HOME/.codex:/tmp/codex" \
  -v "$PWD/benchmarking/result:/workspace/out" \
  graphify-light-bench
```

Add `--model <model>` to the `docker run` command if you want to pin a specific Codex model for all three variants.

The runner copies the repository into disposable work directories before invoking Graphify or graphify-light, so the source checkout is not modified. Results are written to:

| File | Purpose |
|---|---|
| `benchmarking/result/results.md` | Markdown comparison tables for both Codex rounds |
| `benchmarking/result/round-1-understanding.md` | Round 1 table only |
| `benchmarking/result/round-2-follow-up.md` | Round 2 table only |
| `benchmarking/result/results.json` | Structured benchmark data |
| `benchmarking/result/logs/*.jsonl` | Raw Codex JSONL events used to parse token usage |
| `benchmarking/result/answers/*.md` | Final Codex responses |

### Benchmarking Explanation

Latest run: July 1, 2026, inside the Docker benchmark image, using the existing Codex login mounted into the container.

The benchmark separates local preparation from Codex model usage:

- `Direct Codex` gives Codex the raw repository copy.
- `Graphify` first runs `graphify update . --no-cluster`, then gives Codex `graphify-out`.
- `Graphify Light` first runs `graphify-light build`, then gives Codex `.ai/graphify-light`.
- `graphify-light build` does not call an AI model. Its local preparation time was `0.02s` in this run. The token counts in the table are Codex tokens used later to read and answer from the generated graph.
- A no-op Codex initialization baseline was measured first: `10,728` total tokens, including `10,723` input tokens. The table compares task tokens after subtracting that baseline.

Round 1 asked Codex for a repository-understanding report:

| Variant | Task tokens excluding init | Token savings vs Direct | Tokens saved | Local prep seconds | End-to-end seconds | End-to-end time saved |
|---|---:|---:|---:|---:|---:|---:|
| Direct Codex | 218024 | +0.0% | +0 | 0.00 | 72.74 | +0.0% |
| Graphify | 182759 | +16.2% | +35265 | 0.29 | 77.10 | -6.0% |
| Graphify Light | 143004 | +34.4% | +75020 | 0.02 | 74.86 | -2.9% |

Round 2 asked a narrower follow-up architecture question:

| Variant | Task tokens excluding init | Token savings vs Direct | Tokens saved | Local prep seconds | End-to-end seconds | End-to-end time saved |
|---|---:|---:|---:|---:|---:|---:|
| Direct Codex | 100489 | +0.0% | +0 | 0.00 | 47.82 | +0.0% |
| Graphify | 240208 | -139.0% | -139719 | 0.29 | 101.22 | -111.7% |
| Graphify Light | 117780 | -17.2% | -17291 | 0.02 | 48.88 | -2.2% |

Full benchmark tables from [`benchmarking/result/results.md`](benchmarking/result/results.md):

Codex initialization baseline:

| Init total tokens | Init input tokens | Init cached input | Init output tokens | Init reasoning tokens | Init Codex seconds |
|---:|---:|---:|---:|---:|---:|
| 10728 | 10723 | 9088 | 5 | 0 | 5.70 |

Round 1: repository understanding.

| Variant | Status | Task tokens excl. init | Token savings vs Direct | Tokens saved | Local prep seconds | Codex seconds | Codex seconds saved | Codex time saved | End-to-end seconds | End-to-end seconds saved | End-to-end time saved |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Direct Codex | ok | 218024 | +0.0% | +0 | 0.00 | 72.74 | +0.00 | +0.0% | 72.74 | +0.00 | +0.0% |
| Graphify | ok | 182759 | +16.2% | +35265 | 0.29 | 76.81 | -4.07 | -5.6% | 77.10 | -4.36 | -6.0% |
| Graphify Light | ok | 143004 | +34.4% | +75020 | 0.02 | 74.83 | -2.09 | -2.9% | 74.86 | -2.12 | -2.9% |

Round 2: follow-up question.

| Variant | Status | Task tokens excl. init | Token savings vs Direct | Tokens saved | Local prep seconds | Codex seconds | Codex seconds saved | Codex time saved | End-to-end seconds | End-to-end seconds saved | End-to-end time saved |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Direct Codex | ok | 100489 | +0.0% | +0 | 0.00 | 47.82 | +0.00 | +0.0% | 47.82 | +0.00 | +0.0% |
| Graphify | ok | 240208 | -139.0% | -139719 | 0.29 | 100.93 | -53.11 | -111.0% | 101.22 | -53.40 | -111.7% |
| Graphify Light | ok | 117780 | -17.2% | -17291 | 0.02 | 48.86 | -1.03 | -2.2% | 48.88 | -1.06 | -2.2% |

Total token usage across both rounds, including the measured init baseline in each run:

| Variant | Input tokens | Output tokens | Total tokens |
|---|---:|---:|---:|
| Direct Codex | 335056 | 4913 | 339969 |
| Graphify | 436510 | 7913 | 444423 |
| Graphify Light | 276852 | 5388 | 282240 |
