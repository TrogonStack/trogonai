//! Benchmarks the WI-02/WI-04 detectors against the OWASP-ASI-tagged
//! red-team scenarios vendored from AGT's `tests/redteam/test_asi.py`
//! (MIT license; see `fixtures/redteam-asi/PROVENANCE.md`). The upstream
//! Python test drives these scenarios through a `PolicyEvaluator`
//! (CEL/Rego-shaped policy documents) that has no equivalent in this crate;
//! only the scenario *data* (payload text, expected action, ASI risk
//! category) is vendored, per WI-06.
//!
//! Each scenario's `output`/`action` value is treated as the candidate text
//! a detector would scan. Scenarios whose `field` is `"action"` carry a bare
//! action name (e.g. `"register_mcp"`), not natural-language text a
//! description scanner is designed for; those are recorded as
//! `covered: false` in the fixture rather than silently skipped, per the
//! work item's explicit instruction not to hide detector gaps.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::fs;

use mcp_gateway::scan::description_injection::check_description_injection;
use mcp_gateway::scan::hidden_instructions::check_hidden_instructions;
use mcp_gateway::{ServerName, ToolName};

struct Scenario {
    name: String,
    value: String,
    expected_action: String,
    asi_risk: String,
    covered: bool,
    detector: Option<String>,
}

fn load_scenarios() -> Vec<Scenario> {
    let path = format!("{}/fixtures/redteam-asi/scenarios.json", env!("CARGO_MANIFEST_DIR"));
    let contents = fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {path}: {err}"));
    let root: serde_json::Value = serde_json::from_str(&contents).expect("valid JSON");
    let scenarios = root["scenarios"].as_array().expect("scenarios array");
    scenarios
        .iter()
        .map(|s| Scenario {
            name: s["name"].as_str().expect("name").to_string(),
            value: s["value"].as_str().expect("value").to_string(),
            expected_action: s["expected_action"].as_str().expect("expected_action").to_string(),
            asi_risk: s["asi_risk"].as_str().expect("asi_risk").to_string(),
            covered: s["covered"].as_bool().expect("covered"),
            detector: s["detector"].as_str().map(str::to_string),
        })
        .collect()
}

fn tool_name() -> ToolName {
    ToolName::new("scanned-tool").expect("valid tool name")
}

fn server_name() -> ServerName {
    ServerName::new("benchmark-server").expect("valid server name")
}

fn hidden_instructions_flags(text: &str) -> bool {
    !check_hidden_instructions(text, &tool_name(), &server_name()).is_empty()
}

fn description_injection_flags(text: &str) -> bool {
    !check_description_injection(text, &tool_name(), &server_name()).is_empty()
}

fn combined_flags(text: &str) -> bool {
    hidden_instructions_flags(text) || description_injection_flags(text)
}

/// The fixture must vendor all 28 upstream scenarios (27 `deny`-expected
/// adversarial payloads plus 1 `allow`-expected benign baseline), matching
/// `test_asi.py`'s `SCENARIOS` list length, so nothing was dropped in
/// transcription.
#[test]
fn fixture_matches_upstream_scenario_count() {
    let scenarios = load_scenarios();
    assert_eq!(scenarios.len(), 28, "expected 28 vendored ASI red-team scenarios");
    let deny_count = scenarios.iter().filter(|s| s.expected_action == "deny").count();
    let allow_count = scenarios.iter().filter(|s| s.expected_action == "allow").count();
    assert_eq!(deny_count, 27, "expected 27 deny-expected adversarial scenarios");
    assert_eq!(allow_count, 1, "expected 1 allow-expected benign baseline scenario");
}

/// For every scenario marked `covered: true` in the fixture, the detector
/// named in its `detector` field must actually flag the scenario's value.
/// This keeps the fixture's coverage claims honest: if a detector rule
/// changes and stops matching, this test catches the drift instead of
/// leaving a stale `covered: true` in the JSON.
#[test]
fn covered_scenarios_are_actually_flagged_by_their_named_detector() {
    let scenarios = load_scenarios();
    let mut checked = 0;
    for scenario in scenarios.iter().filter(|s| s.covered) {
        let detector = scenario
            .detector
            .as_deref()
            .unwrap_or_else(|| panic!("scenario '{}' is covered=true but has no detector field", scenario.name));
        let flagged = match detector {
            "hidden_instructions" => hidden_instructions_flags(&scenario.value),
            "description_injection" => description_injection_flags(&scenario.value),
            other => panic!("scenario '{}' names unknown detector '{other}'", scenario.name),
        };
        assert!(
            flagged,
            "scenario '{}' (ASI {}) is marked covered by {detector} but was not flagged",
            scenario.name, scenario.asi_risk
        );
        checked += 1;
    }
    println!("[redteam-asi] {checked} scenarios verified as actually covered by their named detector");
    assert!(checked > 0, "expected at least one covered scenario to verify");
}

/// Measures combined detector recall across every `deny`-expected
/// (adversarial) scenario, regardless of the fixture's per-scenario
/// `covered` flag, and prints per-ASI-risk-category detail. This is the
/// same measure-then-assert-a-floor approach as the prompt-injection
/// benchmark: report the true number, then guard against regression.
#[test]
fn redteam_asi_benchmark_combined_detectors() {
    let scenarios = load_scenarios();
    let adversarial: Vec<&Scenario> = scenarios.iter().filter(|s| s.expected_action == "deny").collect();

    let mut caught = 0;
    for scenario in &adversarial {
        let flagged = combined_flags(&scenario.value);
        println!(
            "[redteam-asi] {:<32} asi={:<10} flagged={:<5} covered={}",
            scenario.name, scenario.asi_risk, flagged, scenario.covered
        );
        if flagged {
            caught += 1;
        }
    }

    let recall = caught as f64 / adversarial.len() as f64;
    println!(
        "[redteam-asi combined] recall={recall:.4} ({caught}/{}) across {} adversarial scenarios",
        adversarial.len(),
        adversarial.len()
    );

    // Measured at time of writing: 5/27 adversarial scenarios flagged by the
    // combined detector (recall ~0.1852). This is expected to be low: most
    // scenarios exercise policy-layer concerns (bare action names, business
    // process fraud, account-takeover social engineering, payment
    // redirection) that a lexical MCP-tool-description scanner was never
    // designed to catch; see each scenario's `covered_note` in
    // fixtures/redteam-asi/scenarios.json for the specific reason.
    assert!(
        recall >= 0.17,
        "redteam-asi combined detector recall regressed below measured floor: {recall:.4}"
    );

    // The benign baseline scenario must not be flagged by either detector,
    // matching its `expected_action: allow`.
    let benign = scenarios
        .iter()
        .find(|s| s.expected_action == "allow")
        .expect("fixture has exactly one allow-expected scenario");
    assert!(
        !combined_flags(&benign.value),
        "benign baseline scenario '{}' was incorrectly flagged",
        benign.name
    );
}
