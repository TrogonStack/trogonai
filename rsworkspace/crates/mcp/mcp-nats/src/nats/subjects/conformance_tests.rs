//! ADR#0055 conformance over the whole MCP subject inventory.
//!
//! MCP subjects are generated rather than hand-written: every peer subject is
//! `PeerSubject` over a method terminal, so iterating [`METHOD_TABLE`] plus both
//! roles is exhaustive by construction. A method added to the table is covered
//! here the moment it is added.

use trogon_nats::subject_conformance::{
    looks_like_request_id, validate_binding_version, validate_published_subject, validate_subject_pattern,
};

use super::{METHOD_TABLE, McpRole, PeerSubject, method_from_suffix, method_suffix, subscriptions};
use crate::{McpPeerId, McpPrefix};

const PREFIXES: [&str; 2] = ["mcp", "my.multi.part"];
const ROLES: [McpRole; 2] = [McpRole::Server, McpRole::Client];

fn prefix(s: &str) -> McpPrefix {
    McpPrefix::new(s).expect("test prefix")
}

fn peer() -> McpPeerId {
    McpPeerId::new("peer_01").expect("test peer id")
}

/// Every peer subject the table can produce, plus the escape encoding.
fn published(p: &McpPrefix) -> Vec<String> {
    let id = peer();
    let mut out = Vec::new();

    for role in ROLES {
        for (method, _) in METHOD_TABLE {
            out.push(
                PeerSubject::for_method(p, role, &id, method)
                    .expect("METHOD_TABLE entry must map")
                    .to_string(),
            );
        }
        // The `custom.{base64url}` escape keeps the mapping total, so it is part
        // of the published surface and must clear the same limits.
        out.push(
            PeerSubject::for_method(p, role, &id, "vendor/someUnknownMethod")
                .expect("unknown methods escape rather than fail")
                .to_string(),
        );
    }
    out
}

fn patterns(p: &McpPrefix) -> Vec<String> {
    let id = peer();
    vec![
        subscriptions::AllClientSubject::new(p).to_string(),
        subscriptions::AllServerSubject::new(p).to_string(),
        subscriptions::OneClientSubject::new(p, &id).to_string(),
        subscriptions::OneServerSubject::new(p, &id).to_string(),
    ]
}

#[test]
fn every_published_subject_conforms() {
    for prefix_str in PREFIXES {
        let p = prefix(prefix_str);
        for subject in published(&p) {
            assert_eq!(validate_published_subject(&subject), Ok(()), "{subject}");
        }
    }
}

#[test]
fn every_pattern_conforms() {
    for prefix_str in PREFIXES {
        let p = prefix(prefix_str);
        for subject in patterns(&p) {
            assert_eq!(validate_subject_pattern(&subject), Ok(()), "{subject}");
        }
    }
}

#[test]
fn every_subject_carries_the_binding_version() {
    for prefix_str in PREFIXES {
        let p = prefix(prefix_str);
        for subject in published(&p).into_iter().chain(patterns(&p)) {
            assert_eq!(validate_binding_version(&subject, prefix_str), Ok(()), "{subject}");
        }
    }
}

#[test]
fn no_subject_embeds_a_request_id() {
    // MCP has no per-request subject token, so unlike ACP and A2A this needs no
    // allowlist. It must stay that way.
    let p = prefix("mcp");
    for subject in published(&p) {
        assert!(
            !subject.split('.').any(looks_like_request_id),
            "subject embeds a request id: {subject}"
        );
    }
}

#[test]
fn method_terminals_are_lower_snake_and_round_trip() {
    // ADR#0055 requires the mapping be total and bidirectional, and terminals be
    // lower_snake. `method_suffix` returning a `custom.` escape here would mean
    // the table lost an entry.
    for (method, suffix) in METHOD_TABLE {
        assert_eq!(method_suffix(method).expect("method must map"), *suffix, "{method}");
        assert_eq!(
            method_from_suffix(suffix).expect("suffix must map back"),
            *method,
            "{suffix}"
        );
        assert!(
            suffix.split('.').all(|t| !t.is_empty()
                && t.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')),
            "terminal is not lower_snake: {suffix}"
        );
    }
}
