#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

//! ADR#0055 guardrail: subjects are built by the subject value objects, not by
//! hand at the call site.
//!
//! A subject spelled inline as `format!("{p}.v1.tasks.{id}.events")` bypasses
//! every check the value objects carry: the token and byte budgets, the
//! grammar, and the conformance suites in each binding crate. It also hides the
//! subject from the inventory those suites enumerate, so a nonconforming
//! subject can ship without any test noticing.
//!
//! This runs as a test rather than a dylint because most subjects are built
//! through `format!`, whose template is a macro argument: by the time a late
//! lint pass sees it the string has been split into expansion fragments. The
//! source text is where the policy is actually legible.
//!
//! The rule is enforced as a ratchet, not a clean bill of health. The counts
//! below are the sites that existed when the guardrail was written. Adding a
//! site fails this test; removing one fails it too, until the count is lowered
//! here. The intended end state is an empty table.
//!
//! MCP is already there: it builds every subject through `PeerSubject` over the
//! method table, which is why it has no entry at all.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Binding crates governed by ADR#0055's subject grammar. The scheduler uses a
/// separate subject convention and is out of scope.
const SCANNED_ROOTS: [&str; 3] = ["crates/a2a", "crates/acp", "crates/mcp"];

/// Hand-built subject sites, by file, as of this guardrail landing.
///
/// Never add a row. Lower a count when a site moves onto a subject value
/// object, and delete the row when it reaches zero.
const KNOWN_SITES: [(&str, usize); 13] = [
    ("crates/a2a/a2a-auth-callout/src/permissions.rs", 2),
    ("crates/a2a/a2a-bridge/src/inbound.rs", 1),
    ("crates/a2a/a2a-gateway/src/config.rs", 1),
    ("crates/a2a/a2a-gateway/src/gw_pull_backpressure.rs", 4),
    ("crates/a2a/a2a-gateway/src/push_dlq_mirror.rs", 3),
    ("crates/a2a/a2a-gateway/src/runtime/dispatch.rs", 1),
    ("crates/a2a/a2a-nats/src/audit/emitter.rs", 2),
    ("crates/a2a/a2a-nats/src/catalog/registrar.rs", 5),
    ("crates/a2a/a2a-nats/src/constants.rs", 1),
    ("crates/a2a/a2a-nats/src/gateway_ingress.rs", 8),
    ("crates/a2a/a2a-nats/src/push/dlq.rs", 1),
    ("crates/a2a/a2a-nats/src/server/bridge.rs", 2),
    ("crates/acp/acp-nats/src/jetstream/consumers.rs", 1),
];

fn workspace_root() -> PathBuf {
    // <root>/crates/platform/trogon-nats -> <root>
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(3)
        .expect("manifest dir has a workspace root three levels up")
        .to_path_buf()
}

/// True for a source file whose contents are dictated by a generator.
fn is_generated(text: &str) -> bool {
    text.lines()
        .take(5)
        .any(|line| line.contains("@generated") || line.contains("DO NOT EDIT"))
}

/// Test and test-support sources spell subjects out on purpose: an assertion
/// that a subject renders to an exact string is the point of those tests.
fn is_test_source(path: &str) -> bool {
    path.contains("/tests/")
        || path.ends_with("/tests.rs")
        || path.ends_with("_tests.rs")
        || path.contains("/test_support")
        || path.contains("/mocks")
        || path.contains("/testkit")
        || path.contains("/fixtures")
        || path.contains("_harness")
        || path.contains("/benches/")
}

/// A string literal containing a `.v{major}.` binding-version token, which is
/// the part of the grammar no non-subject string has a reason to carry.
fn looks_like_a_subject_literal(line: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with("//") || trimmed.starts_with('*') {
        return false;
    }
    let bytes = line.as_bytes();
    let mut in_string = false;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' if in_string => i += 1,
            b'"' => in_string = !in_string,
            b'.' if in_string => {
                let rest = &line[i + 1..];
                let digits: String = rest.chars().skip(1).take_while(char::is_ascii_digit).collect();
                if rest.starts_with('v') && !digits.is_empty() && rest[1 + digits.len()..].starts_with('.') {
                    return true;
                }
            }
            _ => {}
        }
        i += 1;
    }
    false
}

fn collect(dir: &Path, root: &Path, out: &mut BTreeMap<String, usize>) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("read {}: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            collect(&path, root, out);
            continue;
        }
        if path.extension().is_none_or(|e| e != "rs") {
            continue;
        }

        let rel = path
            .strip_prefix(root)
            .expect("scanned path is under the workspace root")
            .to_string_lossy()
            .replace('\\', "/");

        // The subject modules are the sanctioned home for this grammar.
        if rel.contains("/nats/subjects/") || is_test_source(&rel) {
            continue;
        }

        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
        if is_generated(&text) {
            continue;
        }

        let count = text.lines().filter(|l| looks_like_a_subject_literal(l)).count();
        if count > 0 {
            out.insert(rel, count);
        }
    }
}

#[test]
fn subjects_are_not_hand_built_outside_the_subject_modules() {
    let root = workspace_root();
    let mut found = BTreeMap::new();
    for scanned in SCANNED_ROOTS {
        let dir = root.join(scanned);
        assert!(dir.is_dir(), "scanned root is missing: {}", dir.display());
        collect(&dir, &root, &mut found);
    }

    let expected: BTreeMap<String, usize> = KNOWN_SITES.iter().map(|(p, n)| ((*p).to_owned(), *n)).collect();

    let added: Vec<_> = found.iter().filter(|(p, _)| !expected.contains_key(*p)).collect();
    assert!(
        added.is_empty(),
        "new hand-built subject sites; build these through a subject value object \
         in `nats/subjects/` instead: {added:#?}"
    );

    assert_eq!(
        found, expected,
        "the hand-built subject inventory changed; lower the KNOWN_SITES count when a \
         site is fixed (and delete the row at zero), and never raise one"
    );
}

#[test]
fn the_detector_recognizes_the_shapes_it_must() {
    for line in [
        r#"    filter_subject: format!("{pfx}.v1.tasks.*.events.*"),"#,
        r#"    let s = "acp.v1.session.s1.agent.prompt";"#,
        r#"    format!("{}.v12.gateway.egress.{}", prefix.as_str(), req_id.as_str())"#,
    ] {
        assert!(looks_like_a_subject_literal(line), "missed: {line}");
    }

    for line in [
        r#"    // acp.v1.session.s1.agent.prompt is the subject"#,
        r#"    use agent_client_protocol::schema::v1::PromptRequest;"#,
        r#"    let version = "1.0";"#,
        r#"    let pkg = trogonai.scheduler.v1.Schedule;"#,
        r#"    let not_a_version = "a.vx.b";"#,
    ] {
        assert!(!looks_like_a_subject_literal(line), "false positive: {line}");
    }
}
