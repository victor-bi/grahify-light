# graphify-light benchmarking

This directory contains a containerized Rust benchmark that compares Codex token usage for the same repository-understanding task across three context strategies:

| Variant | Codex context |
|---|---|
| `direct-codex` | Raw repository files |
| `graphify` | Upstream Graphify output under `graphify-out/` |
| `graphify-light` | `graphify-light` output under `.ai/graphify-light/` |

The runner copies the target repository into `result/work/` before every run, so Graphify and graphify-light artifacts are generated against disposable copies instead of the source checkout.

The Graphify variant runs `graphify update . --no-cluster` inside its disposable corpus copy. That keeps the benchmark headless and focused on Codex query-time token usage rather than Graphify's optional LLM clustering or report-generation cost.

## Build

```bash
docker build -f benchmarking/Dockerfile -t graphify-light-bench .
```

## Run

Use an API key:

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

Or use an existing Codex login:

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

Add `--model <model>` to pin the same Codex model across all three variants.

The benchmark writes:

| File | Purpose |
|---|---|
| `benchmarking/result/results.md` | Markdown comparison tables for both Codex rounds |
| `benchmarking/result/round-1-understanding.md` | Round 1 table only |
| `benchmarking/result/round-2-follow-up.md` | Round 2 table only |
| `benchmarking/result/results.json` | Structured result data |
| `benchmarking/result/logs/*.jsonl` | Raw Codex JSONL events for each variant and round |
| `benchmarking/result/answers/*.md` | Final Codex responses |

## Result Columns

The benchmark first runs a no-op Codex prompt to estimate initialization overhead. The main comparison uses task tokens after subtracting that initialization baseline.

| Column | Meaning |
|---|---|
| `Task tokens excl. init` | Codex total tokens for the task after subtracting the no-op initialization baseline |
| `Token savings vs Direct` | Percent fewer task tokens than `Direct Codex`; negative means more tokens than direct |
| `Tokens saved` | Absolute task-token difference versus `Direct Codex` |
| `Local prep seconds` | Local non-AI preparation time before Codex runs, such as Graphify or graphify-light graph generation |
| `Codex seconds` | Wall-clock time spent in the Codex model call for that row |
| `Codex seconds saved` | Seconds saved versus the Direct Codex model call for the same round |
| `Codex time saved` | Percent Codex-call time saved versus Direct Codex |
| `End-to-end seconds` | Local prep seconds plus Codex seconds |
| `End-to-end seconds saved` | Total seconds saved versus Direct Codex, including local prep |
| `End-to-end time saved` | Percent total time saved versus Direct Codex |

`graphify-light build` is deterministic local Rust static analysis and does not call an AI model. Tokens shown for `Graphify Light` are Codex tokens spent after the graph is generated.

## Latest Result

Run date: July 1, 2026.

Initialization baseline:

| Init total tokens | Init input tokens | Init cached input | Init output tokens | Init reasoning tokens | Init Codex seconds |
|---:|---:|---:|---:|---:|---:|
| 10728 | 10723 | 9088 | 5 | 0 | 5.70 |

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

Interpretation: graphify-light reduced first-round Codex task tokens by `34.4%` with only `0.02s` of local graph build time. It did not improve end-to-end wall-clock time in this run because the Codex call dominated. The second follow-up did not favor graph-based contexts: Graphify used substantially more tokens than direct raw-file Codex, and graphify-light used `17.2%` more task tokens than direct.
