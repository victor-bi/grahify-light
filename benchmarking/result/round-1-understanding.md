# Round 1: repository understanding

Ask Codex to produce a concise repository-understanding report.

Init baseline: 10728 total tokens, 10723 input tokens, 5.70 Codex seconds.

| Variant | Status | Task tokens excl. init | Token savings vs Direct | Tokens saved | Local prep seconds | Codex seconds | Codex seconds saved | Codex time saved | End-to-end seconds | End-to-end seconds saved | End-to-end time saved |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| Direct Codex | ok | 218024 | +0.0% | +0 | 0.00 | 72.74 | +0.00 | +0.0% | 72.74 | +0.00 | +0.0% |
| Graphify | ok | 182759 | +16.2% | +35265 | 0.29 | 76.81 | -4.07 | -5.6% | 77.10 | -4.36 | -6.0% |
| Graphify Light | ok | 143004 | +34.4% | +75020 | 0.02 | 74.83 | -2.09 | -2.9% | 74.86 | -2.12 | -2.9% |
