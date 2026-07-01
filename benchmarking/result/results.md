# Benchmark Results

Codex initialization baseline, measured with a no-op prompt and subtracted from task-token comparisons:

| Init total tokens | Init input tokens | Init cached input | Init output tokens | Init reasoning tokens | Init Codex seconds |
|---:|---:|---:|---:|---:|---:|
| 10728 | 10723 | 9088 | 5 | 0 | 5.70 |

## Round 1: repository understanding

Ask Codex to produce a concise repository-understanding report.

| Variant | Status | Task tokens excl. init | Token savings vs Direct | Tokens saved | Local prep seconds | Codex seconds | Codex seconds saved | Codex time saved | End-to-end seconds | End-to-end seconds saved | End-to-end time saved |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Direct Codex | ok | 218024 | +0.0% | +0 | 0.00 | 72.74 | +0.00 | +0.0% | 72.74 | +0.00 | +0.0% |
| Graphify | ok | 182759 | +16.2% | +35265 | 0.29 | 76.81 | -4.07 | -5.6% | 77.10 | -4.36 | -6.0% |
| Graphify Light | ok | 143004 | +34.4% | +75020 | 0.02 | 74.83 | -2.09 | -2.9% | 74.86 | -2.12 | -2.9% |

## Round 2: follow-up question

Ask Codex a second, narrower architecture question against the same prepared context.

| Variant | Status | Task tokens excl. init | Token savings vs Direct | Tokens saved | Local prep seconds | Codex seconds | Codex seconds saved | Codex time saved | End-to-end seconds | End-to-end seconds saved | End-to-end time saved |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Direct Codex | ok | 100489 | +0.0% | +0 | 0.00 | 47.82 | +0.00 | +0.0% | 47.82 | +0.00 | +0.0% |
| Graphify | ok | 240208 | -139.0% | -139719 | 0.29 | 100.93 | -53.11 | -111.0% | 101.22 | -53.40 | -111.7% |
| Graphify Light | ok | 117780 | -17.2% | -17291 | 0.02 | 48.86 | -1.03 | -2.2% | 48.88 | -1.06 | -2.2% |

## Total token usage across both rounds

Includes the measured init baseline in each run.

| Variant | Input tokens | Output tokens | Total tokens |
|---|---:|---:|---:|
| Direct Codex | 335056 | 4913 | 339969 |
| Graphify | 436510 | 7913 | 444423 |
| Graphify Light | 276852 | 5388 | 282240 |
