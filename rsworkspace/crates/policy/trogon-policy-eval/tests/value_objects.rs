#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod common;

use std::time::Duration;

use common::{agent, chain, node};
use trogon_policy_eval::{
    AgentId, Callee, Chain, ChainError, DelegationTarget, EgressHostName, EgressHostNameError, HoldName,
    IdentifierError, Phase, Port, PortError, Request, RequestError, RuleName, RuleNameError, StalenessBudget,
    StalenessBudgetError, StreamRevision, Subject,
};

#[test]
fn egress_host_accepts_plain_lowercase_dns_names() {
    for host in [
        "example.com",
        "api.example.com",
        "a",
        "xn--bcher-kva.example",
        "10.0.0.1",
        "my-host.internal",
        "a1.b2.c3",
    ] {
        assert!(EgressHostName::new(host).is_ok(), "{host} should be accepted");
    }
}

#[test]
fn egress_host_rejects_everything_that_is_not_an_exact_plain_name() {
    let too_long_label = format!("{}.com", "a".repeat(64));
    let too_long_name = [
        "a".repeat(60),
        "b".repeat(60),
        "c".repeat(60),
        "d".repeat(60),
        "e".repeat(10),
    ]
    .join(".");
    let cases: Vec<(&str, String, EgressHostNameError)> = vec![
        ("empty", String::new(), EgressHostNameError::Empty),
        (
            "wildcard host",
            "*".to_owned(),
            EgressHostNameError::InvalidCharacter { character: '*' },
        ),
        (
            "wildcard label",
            "*.example.com".to_owned(),
            EgressHostNameError::InvalidCharacter { character: '*' },
        ),
        (
            "scheme",
            "https://example.com".to_owned(),
            EgressHostNameError::InvalidCharacter { character: ':' },
        ),
        (
            "path",
            "example.com/path".to_owned(),
            EgressHostNameError::InvalidCharacter { character: '/' },
        ),
        (
            "slash alone",
            "/".to_owned(),
            EgressHostNameError::InvalidCharacter { character: '/' },
        ),
        (
            "embedded port",
            "example.com:443".to_owned(),
            EgressHostNameError::InvalidCharacter { character: ':' },
        ),
        (
            "space",
            "example .com".to_owned(),
            EgressHostNameError::InvalidCharacter { character: ' ' },
        ),
        (
            "userinfo",
            "user@example.com".to_owned(),
            EgressHostNameError::InvalidCharacter { character: '@' },
        ),
        (
            "underscore",
            "my_host.example.com".to_owned(),
            EgressHostNameError::InvalidCharacter { character: '_' },
        ),
        ("uppercase", "Example.com".to_owned(), EgressHostNameError::Uppercase),
        (
            "trailing dot",
            "example.com.".to_owned(),
            EgressHostNameError::TrailingDot,
        ),
        (
            "leading dot",
            ".example.com".to_owned(),
            EgressHostNameError::EmptyLabel,
        ),
        ("double dot", "example..com".to_owned(), EgressHostNameError::EmptyLabel),
        (
            "idn lookalike (cyrillic a)",
            "\u{0430}pple.com".to_owned(),
            EgressHostNameError::NotAscii,
        ),
        (
            "idn unicode name",
            "b\u{00fc}cher.example".to_owned(),
            EgressHostNameError::NotAscii,
        ),
        (
            "leading hyphen",
            "-example.com".to_owned(),
            EgressHostNameError::HyphenAtLabelEdge,
        ),
        (
            "trailing hyphen",
            "example-.com".to_owned(),
            EgressHostNameError::HyphenAtLabelEdge,
        ),
        (
            "label above 63 bytes",
            too_long_label,
            EgressHostNameError::LabelTooLong { actual: 64, limit: 63 },
        ),
        (
            "name above 253 bytes",
            too_long_name.clone(),
            EgressHostNameError::TooLong {
                actual: too_long_name.len(),
                limit: 253,
            },
        ),
    ];
    for (name, input, expected) in cases {
        assert_eq!(EgressHostName::new(input), Err(expected), "case: {name}");
    }
}

#[test]
fn port_accepts_the_valid_range_and_rejects_the_rest() {
    assert_eq!(Port::new(1).map(Port::get), Ok(1));
    assert_eq!(Port::new(65535).map(Port::get), Ok(65535));
    assert_eq!(Port::new(0), Err(PortError::Zero));
    assert_eq!(Port::try_from(0_u32), Err(PortError::Zero));
    assert_eq!(Port::try_from(443_u32).map(Port::get), Ok(443));
    assert_eq!(Port::try_from(65535_u32).map(Port::get), Ok(65535));
    assert_eq!(Port::try_from(65536_u32), Err(PortError::OutOfRange(65536)));
    assert_eq!(Port::try_from(u32::MAX), Err(PortError::OutOfRange(u32::MAX)));
}

#[test]
fn identifiers_reject_empty_oversized_and_ambiguous_values() {
    assert_eq!(AgentId::new(""), Err(IdentifierError::Empty));
    assert_eq!(
        AgentId::new("has space"),
        Err(IdentifierError::ContainsWhitespaceOrControl)
    );
    assert_eq!(
        AgentId::new("tab\there"),
        Err(IdentifierError::ContainsWhitespaceOrControl)
    );
    assert_eq!(
        AgentId::new("new\nline"),
        Err(IdentifierError::ContainsWhitespaceOrControl)
    );
    assert_eq!(
        AgentId::new("a".repeat(257)),
        Err(IdentifierError::TooLong {
            actual: 257,
            limit: 256
        })
    );
    assert!(AgentId::new("a".repeat(256)).is_ok());
    assert_eq!(
        HoldName::new("security-incident-42").map(|n| n.to_string()),
        Ok("security-incident-42".to_owned())
    );
}

#[test]
fn identifiers_are_exact_and_case_sensitive() {
    let lower = AgentId::new("agent-a").expect("valid");
    let upper = AgentId::new("Agent-A").expect("valid");
    assert_ne!(lower, upper);
    assert_eq!(lower.as_str(), "agent-a");
    assert_eq!("agent-a".parse::<AgentId>(), Ok(lower));
}

#[test]
fn egress_host_reads_back_as_stored() {
    assert_eq!(common::host("api.example.com").as_str(), "api.example.com");
}

#[test]
fn a_subject_is_built_from_either_identity_and_names_which_it_is() {
    let from_agent = Subject::from(common::agent_id("a"));
    let from_node = Subject::from(common::node_id("team"));
    assert_eq!(from_agent, agent("a"));
    assert_eq!(from_node, node("team"));
    assert_eq!(from_agent.to_string(), "agent a");
    assert_eq!(from_node.to_string(), "node team");
}

#[test]
fn rule_names_reject_empty_whitespace_only_oversized_and_control_values() {
    assert_eq!(RuleName::new(""), Err(RuleNameError::Empty));
    assert_eq!(RuleName::new("   "), Err(RuleNameError::Empty));
    assert_eq!(
        RuleName::new("a".repeat(257)),
        Err(RuleNameError::TooLong {
            actual: 257,
            limit: 256
        })
    );
    assert_eq!(
        RuleName::new("bad\u{7}bell"),
        Err(RuleNameError::ContainsControlCharacter)
    );
    assert_eq!(
        RuleName::new("block exfil").map(|name| name.to_string()),
        Ok("block exfil".to_owned())
    );
}

#[test]
fn chain_is_non_empty_deduplicated_and_nearest_first() {
    assert_eq!(Chain::new(Vec::new()), Err(ChainError::Empty));

    let built = chain(vec![agent("a"), node("team"), node("team"), agent("a"), node("org")]);
    let subjects: Vec<_> = built.iter().cloned().collect();
    assert_eq!(subjects, vec![agent("a"), node("team"), node("org")]);
    assert_eq!(built.subject(), &agent("a"));
    assert!(built.contains(&node("org")));
    assert!(!built.contains(&node("other")));
}

#[test]
fn callee_knows_which_nodes_it_is_placed_under() {
    let callee = Callee::new(
        AgentId::new("b").expect("valid"),
        [common::node_id("team"), common::node_id("org")],
    );
    assert!(callee.is_placed_under(&common::node_id("org")));
    assert!(!callee.is_placed_under(&common::node_id("elsewhere")));
    assert_eq!(callee.agent().as_str(), "b");
}

#[test]
fn delegation_targets_cover_exact_agents_and_nodes_above_them() {
    let callee = Callee::new(common::agent_id("b"), [common::node_id("team")]);
    assert!(DelegationTarget::Agent(common::agent_id("b")).covers(&callee));
    assert!(!DelegationTarget::Agent(common::agent_id("c")).covers(&callee));
    assert!(DelegationTarget::AgentsUnder(common::node_id("team")).covers(&callee));
    assert!(!DelegationTarget::AgentsUnder(common::node_id("other")).covers(&callee));
    // A node named like the agent is a different identity.
    assert!(!DelegationTarget::AgentsUnder(common::node_id("b")).covers(&callee));
}

#[test]
fn request_refuses_a_lineage_that_omits_part_of_the_own_chain() {
    let own = chain(vec![agent("a"), node("team")]);
    let lineage = chain(vec![agent("a"), node("other")]);
    let result = Request::new(
        own.clone(),
        lineage,
        trogon_policy_eval::Action::ModelUse {
            model: common::model("m"),
        },
        Phase::ProtectedAction,
    );
    assert_eq!(
        result,
        Err(RequestError::LineageMissesOwnChain { missing: node("team") })
    );

    let ok = Request::new(
        own.clone(),
        chain(vec![agent("root"), agent("a"), node("team")]),
        trogon_policy_eval::Action::ModelUse {
            model: common::model("m"),
        },
        Phase::ProtectedAction,
    );
    assert!(ok.is_ok());
}

#[test]
fn request_without_delegation_uses_the_own_chain_as_the_lineage() {
    let request = common::egress(common::chain_of_a(), "example.com", 443);
    assert_eq!(request.own_chain(), request.lineage());
}

#[test]
fn staleness_budget_never_exceeds_the_ceiling() {
    assert_eq!(StalenessBudget::default().get(), Duration::from_secs(330));
    assert_eq!(StalenessBudget::ceiling().get(), Duration::from_secs(330));
    assert_eq!(
        StalenessBudget::new(Duration::from_secs(5)).map(StalenessBudget::get),
        Ok(Duration::from_secs(5))
    );
    assert_eq!(
        StalenessBudget::new(Duration::from_secs(330)).map(StalenessBudget::get),
        Ok(Duration::from_secs(330))
    );
    assert_eq!(StalenessBudget::new(Duration::ZERO), Err(StalenessBudgetError::Zero));
    assert_eq!(
        StalenessBudget::new(Duration::from_secs(331)),
        Err(StalenessBudgetError::AboveCeiling {
            requested: Duration::from_secs(331),
            ceiling: Duration::from_secs(330),
        })
    );
    assert!(StalenessBudget::new(Duration::from_millis(330_001)).is_err());
}

#[test]
fn stream_revision_zero_means_the_history_did_not_exist() {
    assert!(StreamRevision::missing().is_missing());
    assert_eq!(StreamRevision::missing().get(), 0);
    assert!(!StreamRevision::new(1).is_missing());
    assert!(StreamRevision::new(2) > StreamRevision::new(1));
}
