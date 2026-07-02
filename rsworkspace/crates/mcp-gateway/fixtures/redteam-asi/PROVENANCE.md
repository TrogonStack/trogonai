# Provenance: OWASP-ASI-tagged red-team scenarios

Vendored from Microsoft's Agent Governance Toolkit (AGT), MIT license.
See `fixtures/LICENSE-MIT` for the full license text and copyright notice.

- Upstream path: `tests/redteam/test_asi.py` (`SCENARIOS` list).
- Upstream commit: the checkout under `tmp/agent-governance-toolkit` at the
  time of vendoring (WI-06, `MS_AGENT_GOV_TOOLKIT_WORKITEMS.md`).
- `scenarios.json` transcribes the 28 `AdversarialScenario` dataclass
  instances (`name`, `field`, `value`, `expected_action`, `asi_risk`,
  `intelligence_source`) verbatim as JSON objects. 27 carry
  `expected_action: "deny"` (adversarial payloads mapped to OWASP Agentic
  Security Initiative risk categories ASI-01 through ASI-10); 1
  (`Benign-Read-Operation`) carries `expected_action: "allow"` and served
  upstream as the negative-control baseline.
- Only the scenario *data* is vendored. Upstream's `test_asi.py` drives
  each scenario through a `PolicyEvaluator` evaluating YAML policy packs
  (`templates/policies/starters/{healthcare,financial-services,general-saas}.yaml`)
  with a `PolicyDocument`/`PolicyEvaluator` implementation this repo has no
  equivalent of; that harness logic does not transfer and is not vendored.

## Added fields not present upstream

Each scenario in `scenarios.json` additionally carries:

- `covered` (bool): whether this crate's `hidden_instructions` or
  `description_injection` detector (WI-02/WI-04 scope) actually flags the
  scenario's `value` text, as measured by
  `tests/redteam_asi_benchmark.rs::covered_scenarios_are_actually_flagged_by_their_named_detector`.
  Per WI-06, scenarios no detector covers are recorded here with
  `covered: false` rather than silently dropped from the fixture.
- `detector` (string, present only when `covered: true`): which detector
  (`hidden_instructions` or `description_injection`) flags the scenario.
- `covered_note` (string): a short explanation of why the scenario is or
  is not covered, naming the specific matched rule or the specific gap.

These fields describe this crate's current detector coverage; they are not
part of AGT's original scenario data and are re-verified by the benchmark
test on every run, so a rule change that breaks a `covered: true` claim
fails CI instead of leaving a stale claim in the fixture.

## What this crate does with it

`tests/redteam_asi_benchmark.rs` loads every scenario, asserts the fixture
still has all 28 upstream rows, verifies every `covered: true` claim is
still true, and measures combined-detector recall across the 27
`deny`-expected adversarial scenarios (regardless of the `covered` flag),
printing per-scenario and aggregate results. As with the prompt-injection
benchmark, the asserted recall floor is a regression guard derived from
what was actually measured, not a claim that these detectors are a
complete ASI-risk mitigation.
