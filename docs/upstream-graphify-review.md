# Upstream Graphify Review

Reviewed on 2026-07-03 against:

- Repository: <https://github.com/safishamsi/graphify>
- Local review commit: `cf4b4ef85a72c407b5e1cb5e0678faa0497a2747`
- Files inspected: `graphify/detect.py`, `graphify/extract.py`, `graphify/manifest_ingest.py`, `graphify/mcp_ingest.py`, `graphify/hooks.py`, `graphify/__main__.py`, and related tests.

## Upstream Detection

Upstream Graphify classifies files into code, document, paper, image, and video buckets. It recognizes a broad source set: Python, JavaScript, TypeScript, TSX, Go, Rust, Java, Groovy/Gradle, C/C++, Ruby, Swift, Kotlin, C#, Scala, PHP, Lua/Luau, Zig, PowerShell, Elixir, Objective-C, Julia, Vue, Svelte, Astro, Dart, Verilog/SystemVerilog, SQL, R, Fortran, Pascal/Delphi, BYOND DM, .NET project files, Apex, JSON, Terraform/HCL, and shell scripts.

It also detects Markdown/text/HTML/YAML documents, PDFs, images, Office files, Google Workspace shortcuts, and audio/video. Office and PDF paths have local safety caps. Sensitive files are skipped by directory and filename patterns, including private keys, env files, credential stores, and load-bearing secret/token names.

`graphify-light` now mirrors the broad local detection shape for token-free indexing. It uses `.gitignore` through the Rust `ignore` crate, skips dependency/build/noise directories, and keeps stricter local size caps for source, known resources, and unknown resources.

## Upstream Extraction

Upstream code extraction is primarily deterministic Tree-sitter extraction, backed by a language registry-like `LanguageConfig` model plus special extractors. The current upstream dispatch includes dedicated handling for:

- Generic Tree-sitter languages such as Python, JS/TS/TSX, Java, Groovy, C/C++, Ruby, C#, Kotlin, Scala, PHP, Lua, Swift.
- Standalone or specialized extractors for Zig, PowerShell, Elixir, Objective-C, Julia, Fortran, Vue, Svelte, Astro, Dart, Verilog, SQL, Pascal/Delphi, BYOND DM, .NET/XAML/Razor, Apex, Bash, JSON config, Markdown, and Terraform/HCL.
- Cross-file resolution passes for imports, type references, member calls, and ambiguous call targets.

`graphify-light` keeps full Tree-sitter extraction for the Rust crate dependencies it already carries: Python, JavaScript, TypeScript, TSX, Rust, Go, Java, C, and C++. Other source formats are registered and indexed with conservative deterministic heuristics until their Rust grammar crates are added.

## Special Extractors

Upstream routes important non-source files away from the LLM path when deterministic structure is available:

- `manifest_ingest.py` parses package manifests such as `apm.yml`, `pyproject.toml`, `go.mod`, and `pom.xml` into package/dependency graph data.
- `mcp_ingest.py` parses `.mcp.json`, `mcp.json`, `mcp_servers.json`, and `claude_desktop_config.json`; it indexes server names, commands, packages, and environment variable names while never persisting env values.
- `extract_json` indexes only config-shaped JSON and skips data-shaped JSON to avoid exploding datasets into key nodes.
- `extract_terraform` creates resource/data/module/variable/output/provider/local nodes and deterministic `references` or `depends_on` edges.
- `extract_markdown` creates heading nodes and local document reference edges.

`graphify-light` implements these same categories in Rust with a token-free default:

- Package manifests: `package.json`, `pyproject.toml`, `go.mod`, `pom.xml`, `Cargo.toml`, `composer.json`, and APM YAML.
- MCP configs: server, command, package, and env-var-name nodes; env values are ignored.
- JSON/JSONC, TOML, and YAML config extraction with caps; data JSON is file-node-only.
- Kubernetes YAML resource nodes and references to ConfigMaps, Secrets, ServiceAccounts, and PVCs when present in the same file.
- Terraform/HCL block nodes plus deterministic references.
- Markdown/HTML/TXT/RST document headings and local references.
- PDF, Office, image, audio/video, archive, and unknown-binary resource metadata.

## Modes And AI Boundary

Upstream Graphify has deterministic AST extraction, but its full pipeline can use assistant LLMs for semantic document, paper, image, and video extraction. The reviewed code and site describe source-code parsing as local Tree-sitter work, while semantic multimedia extraction is outside the AST-only path.

`graphify-light` now has `.graphify-light.toml` with three modes:

- `none`: default; deterministic Rust extraction and local metadata only.
- `local_model`: reserved for local OCR/speech/vision extractors; currently degrades to `none` without downloads.
- `llm`: requires explicit config enablement; currently degrades to `none` and does not call an LLM.

The graph records `extraction_mode`, `extractor`, and `degraded_reason` metadata so callers can see what actually ran. The default path fails closed: no model download, no remote API, and no token usage.

## Install, Uninstall, Hooks

Upstream Graphify installs skills and assistant guidance across many platforms. Its uninstall path removes managed skill files and managed sections while preserving user-authored content. `hooks.py` manages git post-commit and post-checkout hooks with start/end markers and a pinned Python launcher strategy.

`graphify-light` remains Codex-focused:

- `install codex` writes managed blocks into project or global Codex config and AGENTS guidance.
- `uninstall` removes only graphify-light managed blocks by default.
- `uninstall --global` targets global Codex config and global AGENTS guidance.
- `uninstall --purge` removes `.ai/graphify-light/`.
- Source files and user content outside managed markers are preserved.

## Intentional Deviations

`graphify-light` does not implement upstream Graphify's semantic subagent workflow, LLM extraction, embeddings, Leiden/community analysis, reports, HTML visualization, Obsidian export, Neo4j/FalkorDB integration, Google Workspace export, or multi-assistant skill installation.

The Rust graph keeps the same practical shape of nodes, edges, confidence labels, source provenance, and queryable JSON, but its IDs are deterministic Rust IDs based on repo-relative paths and registered extractor semantics rather than byte-identical upstream IDs.
