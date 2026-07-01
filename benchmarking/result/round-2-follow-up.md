# Round 2: follow-up question

Ask Codex a second, narrower architecture question against the same prepared context.

Init baseline: 10728 total tokens, 10723 input tokens, 5.70 Codex seconds.

| Variant | Status | Task tokens excl. init | Token savings vs Direct | Tokens saved | Local prep seconds | Codex seconds | Codex seconds saved | Codex time saved | End-to-end seconds | End-to-end seconds saved | End-to-end time saved |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Direct Codex | ok | 100489 | +0.0% | +0 | 0.00 | 47.82 | +0.00 | +0.0% | 47.82 | +0.00 | +0.0% |
| Graphify | ok | 240208 | -139.0% | -139719 | 0.29 | 100.93 | -53.11 | -111.0% | 101.22 | -53.40 | -111.7% |
| Graphify Light | ok | 117780 | -17.2% | -17291 | 0.02 | 48.86 | -1.03 | -2.2% | 48.88 | -1.06 | -2.2% |
