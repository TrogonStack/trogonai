# Provenance: prompt-injection corpus

Vendored from Microsoft's Agent Governance Toolkit (AGT), MIT license.
See `fixtures/LICENSE-MIT` for the full license text and copyright notice.

- Upstream path: `benchmarks/prompt-injection/corpus/injection-smoke.jsonl`
  and `benchmarks/prompt-injection/corpus/manifest-smoke.json`.
- Upstream commit: the checkout under `tmp/agent-governance-toolkit` at the
  time of vendoring (WI-06, `MS_AGENT_GOV_TOOLKIT_WORKITEMS.md`).
- `injection-smoke.jsonl` is vendored byte-for-byte, unmodified. Its
  SHA-256 (`f26c216eaf8ad3e623b32a696f6686f69a092e153ec2eae1acb9d8c59f21eca5`)
  matches the `output_sha256` field recorded in `manifest-smoke.json` below,
  confirming nothing was altered in transit.
- `manifest-smoke.json` is vendored unmodified and documents the corpus's
  own generation process, label taxonomy, and row counts (280 total: 110
  attack-labelled, 170 benign-labelled), split assignment, and duplicate/
  leakage checks performed upstream.

## Row shape

Each line of `injection-smoke.jsonl` is one JSON object (JSON Lines, not a
JSON array) with at least these fields used by this crate's tests:

- `id`: unique row id.
- `text`: the candidate prompt/content string to scan.
- `attack_class`: `"benign"` for benign rows, otherwise one of the AGT
  attack-family labels (`direct_override`, `indirect_injection`,
  `tool_abuse`, `prompt_leakage`, `output_exfiltration`,
  `data_boundary_abuse`, `memory_poisoning`, `tool_result_injection`).
- `bypass_class`: obfuscation technique applied to attack rows (`none` for
  rows with no obfuscation, including all benign rows).

See `manifest-smoke.json` for the complete field/count inventory upstream
validated (benign subclasses, source types, trust levels, split
membership, expected actions).

## What this crate does with it

`tests/prompt_injection_benchmark.rs` loads every row, runs it through this
crate's `hidden_instructions` and `description_injection` detectors (the
WI-02/WI-04 scanners), and reports measured recall (attack rows that
produce at least one threat) and false-positive rate (benign rows that
produce at least one threat) per detector and combined. This is an
evaluation fixture only: no runtime behavior in this crate depends on the
corpus, and the thresholds asserted in that test are regression floors
derived from what was actually measured, not a production capability
claim.
