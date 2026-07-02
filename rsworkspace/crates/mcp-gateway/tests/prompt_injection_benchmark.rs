//! Benchmarks the WI-02/WI-04 text-pattern detectors (`hidden_instructions`
//! and `description_injection`) against AGT's vendored 280-row labelled
//! prompt-injection corpus (`fixtures/prompt-injection/injection-smoke.jsonl`,
//! MIT license; see `fixtures/prompt-injection/PROVENANCE.md`).
//!
//! This is an evaluation harness, not a production capability claim: the
//! detectors were built to scan MCP tool descriptions for a specific set of
//! lexical patterns (instruction-override phrases, invisible Unicode,
//! encoded payloads, role-override/exfiltration/privilege-escalation
//! phrases), not to be a general-purpose prompt-injection classifier. This
//! test measures what they actually score on a corpus built for a different,
//! broader detector and asserts regression floors slightly inside the
//! measured numbers, so a future change that quietly guts detection quality
//! fails CI instead of going unnoticed.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::fs;
use std::path::Path;

use mcp_gateway::scan::description_injection::check_description_injection;
use mcp_gateway::scan::hidden_instructions::check_hidden_instructions;
use mcp_gateway::{ServerName, ToolName};

struct CorpusRow {
    id: String,
    text: String,
    is_attack: bool,
}

fn fixture_path(relative: &str) -> String {
    format!("{}/fixtures/{relative}", env!("CARGO_MANIFEST_DIR"))
}

/// Hand-rolled JSON Lines reader: each line is one JSON object, parsed with
/// `serde_json::Value` (already a direct dependency of this crate) rather
/// than deriving a struct, since the test only reads three of the corpus's
/// many labelled fields.
fn load_corpus(path: &Path) -> Vec<CorpusRow> {
    let contents = fs::read_to_string(path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()));
    contents
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let value: serde_json::Value =
                serde_json::from_str(line).unwrap_or_else(|err| panic!("parse corpus row: {err}\nline: {line}"));
            let id = value["id"].as_str().expect("row has string id").to_string();
            let text = value["text"].as_str().expect("row has string text").to_string();
            let attack_class = value["attack_class"].as_str().expect("row has string attack_class");
            CorpusRow {
                id,
                text,
                is_attack: attack_class != "benign",
            }
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

struct Rates {
    attack_total: usize,
    attack_caught: usize,
    benign_total: usize,
    benign_flagged: usize,
}

impl Rates {
    fn recall(&self) -> f64 {
        if self.attack_total == 0 {
            0.0
        } else {
            self.attack_caught as f64 / self.attack_total as f64
        }
    }

    fn false_positive_rate(&self) -> f64 {
        if self.benign_total == 0 {
            0.0
        } else {
            self.benign_flagged as f64 / self.benign_total as f64
        }
    }
}

fn measure(rows: &[CorpusRow], flags: impl Fn(&str) -> bool) -> Rates {
    let mut rates = Rates {
        attack_total: 0,
        attack_caught: 0,
        benign_total: 0,
        benign_flagged: 0,
    };
    for row in rows {
        let flagged = flags(&row.text);
        if row.is_attack {
            rates.attack_total += 1;
            if flagged {
                rates.attack_caught += 1;
            }
        } else {
            rates.benign_total += 1;
            if flagged {
                rates.benign_flagged += 1;
            }
        }
    }
    rates
}

/// Loads the corpus once and asserts its vendored shape matches the AGT
/// upstream manifest (280 rows: 110 attack-labelled, 170 benign-labelled),
/// so a future accidental truncation or corruption of the vendored fixture
/// fails loudly here rather than silently shrinking the benchmark sample.
#[test]
fn corpus_matches_upstream_row_counts() {
    let rows = load_corpus(Path::new(&fixture_path("prompt-injection/injection-smoke.jsonl")));
    assert_eq!(
        rows.len(),
        280,
        "expected 280 total rows, vendored fixture may be truncated"
    );
    let attack = rows.iter().filter(|r| r.is_attack).count();
    let benign = rows.iter().filter(|r| !r.is_attack).count();
    assert_eq!(attack, 110, "expected 110 attack-labelled rows");
    assert_eq!(benign, 170, "expected 170 benign-labelled rows");
    // Every row must carry a stable id; used for reporting mismatches only.
    assert!(rows.iter().all(|r| !r.id.is_empty()));
}

/// Measures `hidden_instructions` alone. This is the higher-recall,
/// higher-false-positive detector of the two: its instruction-override
/// phrase rules (`ignore previous`, `disregard prior`, `system:`, ...)
/// happen to match both live attack rows and benign rows that quote or
/// discuss those same phrases (security documentation, training material,
/// changelogs), since the phrase matcher cannot distinguish an instruction
/// from a mention of one.
#[test]
fn hidden_instructions_benchmark() {
    let rows = load_corpus(Path::new(&fixture_path("prompt-injection/injection-smoke.jsonl")));
    let rates = measure(&rows, hidden_instructions_flags);

    println!(
        "[hidden_instructions] recall={:.4} ({}/{}) fp_rate={:.4} ({}/{})",
        rates.recall(),
        rates.attack_caught,
        rates.attack_total,
        rates.false_positive_rate(),
        rates.benign_flagged,
        rates.benign_total
    );

    // Measured at time of writing: recall 0.2455 (27/110), FP rate 0.1706
    // (29/170). The false-positive rate is high because several benign rows
    // are themselves security-discussion/documentation text that quotes
    // instruction-override phrases (e.g. "explains why 'ignore all previous
    // instructions' is a prompt-injection example"): the detector's phrase
    // matcher has no way to distinguish a quoted example from a live
    // instruction. Thresholds are set a hair below/above the measured
    // values so a regression in matching (e.g. an accidentally narrowed
    // rule) fails this test, without asserting more capability than was
    // actually observed.
    assert!(
        rates.recall() >= 0.23,
        "hidden_instructions recall regressed below measured floor: {:.4}",
        rates.recall()
    );
    assert!(
        rates.false_positive_rate() <= 0.19,
        "hidden_instructions false-positive rate regressed above measured ceiling: {:.4}",
        rates.false_positive_rate()
    );
}

/// Measures `description_injection` alone. This detector's role-override
/// and exfiltration/privilege-escalation phrase lists overlap more with the
/// corpus's `direct_override`, `output_exfiltration`, and `tool_abuse`
/// attack classes, so recall is expected to be higher than
/// `hidden_instructions` alone but still well under 1.0 since the corpus
/// includes obfuscated (leetspeak, homoglyph, ROT13, encoding) variants this
/// detector's plain-word matcher does not decode.
#[test]
fn description_injection_benchmark() {
    let rows = load_corpus(Path::new(&fixture_path("prompt-injection/injection-smoke.jsonl")));
    let rates = measure(&rows, description_injection_flags);

    println!(
        "[description_injection] recall={:.4} ({}/{}) fp_rate={:.4} ({}/{})",
        rates.recall(),
        rates.attack_caught,
        rates.attack_total,
        rates.false_positive_rate(),
        rates.benign_flagged,
        rates.benign_total
    );

    // Measured at time of writing: recall 0.0273 (3/110), FP rate 0.0000
    // (0/170). Recall is low because this corpus's attack rows mostly use
    // paraphrases, obfuscation (leetspeak, homoglyphs, ROT13, encoding), and
    // attack families (prompt leakage, memory poisoning, tool-result
    // injection) that do not use this detector's specific role-override /
    // exfiltration / privilege-escalation phrase list; the corpus was built
    // to stress a broader, unrelated `PromptInjectionDetector`, not this
    // crate's narrower MCP-description scanners.
    assert!(
        rates.recall() >= 0.02,
        "description_injection recall regressed below measured floor: {:.4}",
        rates.recall()
    );
    assert!(
        rates.false_positive_rate() <= 0.01,
        "description_injection false-positive rate regressed above measured ceiling: {:.4}",
        rates.false_positive_rate()
    );
}

/// Measures the two detectors combined (a row counts as flagged if either
/// fires), the shape a caller wiring both into `tools/list` scanning would
/// actually observe.
#[test]
fn combined_detectors_benchmark() {
    let rows = load_corpus(Path::new(&fixture_path("prompt-injection/injection-smoke.jsonl")));
    let rates = measure(&rows, combined_flags);

    println!(
        "[combined hidden_instructions + description_injection] recall={:.4} ({}/{}) fp_rate={:.4} ({}/{})",
        rates.recall(),
        rates.attack_caught,
        rates.attack_total,
        rates.false_positive_rate(),
        rates.benign_flagged,
        rates.benign_total
    );

    // Measured at time of writing: recall 0.2727 (30/110), FP rate 0.1706
    // (29/170).
    assert!(
        rates.recall() >= 0.25,
        "combined detector recall regressed below measured floor: {:.4}",
        rates.recall()
    );
    assert!(
        rates.false_positive_rate() <= 0.19,
        "combined detector false-positive rate regressed above measured ceiling: {:.4}",
        rates.false_positive_rate()
    );
}
