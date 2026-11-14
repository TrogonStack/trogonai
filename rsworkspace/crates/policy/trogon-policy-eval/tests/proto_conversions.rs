#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

mod common;

use common::{
    agent, agent_id, chain_of_a, delegation_permission, destination, egress as egress_request, egress_permission,
    enforce, fresh_now, hold, model_ceiling, node, node_id, stamped_at, view,
};
use trogon_policy_eval::{
    AgentId, DelegationPermission, DelegationTarget, Effective, EgressDestination, EgressHostNameError,
    EgressPermission, Hold, HoldName, HoldScope, IdentifierError, ModelCeiling, ModelId, NodeId, PolicySnapshot,
    PortError, ProtoConversionError, RuleKind, RuleRef, StreamRevision, Subject,
};
use trogonai_proto::agents::agents::v1alpha1 as agents;
use trogonai_proto::hierarchy::v1alpha1 as hierarchy;
use trogonai_proto::policy::delegation_permissions::v1alpha1 as delegation;
use trogonai_proto::policy::egress_permissions::v1alpha1 as egress;
use trogonai_proto::policy::holds::v1alpha1 as holds;
use trogonai_proto::policy::model_ceilings::v1alpha1 as ceilings;
use trogonai_proto::policy::subjects::v1alpha1 as subjects;
use trogonai_proto::policy::subjects::v1alpha1::PolicyRuleKind as Kind;

fn proto_agent(value: &str) -> agents::AgentId {
    agents::AgentId {
        value: value.to_owned(),
    }
}

fn proto_node(value: &str) -> hierarchy::NodeId {
    hierarchy::NodeId {
        value: value.to_owned(),
    }
}

fn proto_agent_subject(value: &str) -> subjects::PolicySubject {
    subjects::PolicySubject {
        subject: proto_agent(value).into(),
    }
}

fn proto_node_subject(value: &str) -> subjects::PolicySubject {
    subjects::PolicySubject {
        subject: proto_node(value).into(),
    }
}

fn proto_host(host: &str, port: u32) -> egress::EgressHost {
    egress::EgressHost {
        host: host.to_owned(),
        port,
    }
}

fn proto_destination(host: &str, port: u32) -> egress::EgressDestination {
    egress::EgressDestination {
        destination: proto_host(host, port).into(),
    }
}

fn proto_egress_policy(destinations: &[(&str, u32)]) -> egress::EgressPermissionPolicy {
    egress::EgressPermissionPolicy {
        permitted: destinations.iter().map(|(h, p)| proto_destination(h, *p)).collect(),
    }
}

fn proto_target_agent(value: &str) -> delegation::DelegationTarget {
    delegation::DelegationTarget {
        target: proto_agent(value).into(),
    }
}

fn proto_target_under(value: &str) -> delegation::DelegationTarget {
    delegation::DelegationTarget {
        target: delegation::__buffa::oneof::delegation_target::Target::AgentsUnder(Box::new(proto_node(value))).into(),
    }
}

fn proto_matcher(value: &str) -> ceilings::ModelMatcher {
    ceilings::ModelMatcher {
        matcher: ceilings::__buffa::oneof::model_matcher::Matcher::ModelId(value.to_owned()).into(),
    }
}

fn proto_hold(name: &str, scope: holds::HoldScope) -> holds::Hold {
    holds::Hold {
        name: holds::HoldName { value: name.to_owned() }.into(),
        scope: scope.into(),
    }
}

fn proto_rule_ref(kind: Kind, subject: subjects::PolicySubject, revision: u64) -> subjects::PolicyRuleRef {
    subjects::PolicyRuleRef {
        kind: kind.into(),
        subject: subject.into(),
        stream_revision: revision,
    }
}

// Identities

#[test]
fn agent_id_converts_and_refuses_an_invalid_identity() {
    assert_eq!(AgentId::try_from(&proto_agent("a")), Ok(agent_id("a")));
    assert_eq!(
        AgentId::try_from(&proto_agent("")),
        Err(ProtoConversionError::InvalidAgentId(IdentifierError::Empty))
    );
}

#[test]
fn node_id_converts_and_refuses_an_invalid_identity() {
    assert_eq!(NodeId::try_from(&proto_node("n")), Ok(node_id("n")));
    assert_eq!(
        NodeId::try_from(&proto_node("a b")),
        Err(ProtoConversionError::InvalidNodeId(
            IdentifierError::ContainsWhitespaceOrControl
        ))
    );
}

// Subject

#[test]
fn subject_converts_both_arms() {
    assert_eq!(Subject::try_from(&proto_agent_subject("a")), Ok(agent("a")));
    assert_eq!(Subject::try_from(&proto_node_subject("n")), Ok(node("n")));
}

#[test]
fn subject_refuses_an_unset_arm_and_an_invalid_identity() {
    assert_eq!(
        Subject::try_from(&subjects::PolicySubject::default()),
        Err(ProtoConversionError::MissingField {
            message: "PolicySubject",
            field: "subject",
        })
    );
    assert_eq!(
        Subject::try_from(&proto_agent_subject("")),
        Err(ProtoConversionError::InvalidAgentId(IdentifierError::Empty))
    );
    assert_eq!(
        Subject::try_from(&proto_node_subject("")),
        Err(ProtoConversionError::InvalidNodeId(IdentifierError::Empty))
    );
}

// RuleKind and RuleRef

#[test]
fn rule_kind_converts_the_kinds_the_evaluator_reads() {
    assert_eq!(RuleKind::try_from(Kind::Hold), Ok(RuleKind::Hold));
    assert_eq!(RuleKind::try_from(Kind::ModelCeiling), Ok(RuleKind::ModelCeiling));
    assert_eq!(
        RuleKind::try_from(Kind::DelegationPermission),
        Ok(RuleKind::DelegationPermission)
    );
    assert_eq!(
        RuleKind::try_from(Kind::EgressPermission),
        Ok(RuleKind::EgressPermission)
    );
}

#[test]
fn rule_kind_refuses_unspecified_and_kinds_it_does_not_read() {
    assert_eq!(
        RuleKind::try_from(Kind::Unspecified),
        Err(ProtoConversionError::UnknownEnumValue {
            enumeration: "PolicyRuleKind",
            value: 0,
        })
    );
    for (kind, value) in [
        (Kind::AgentBudget, 1),
        (Kind::AgentRateLimit, 2),
        (Kind::ConnectionPermission, 6),
    ] {
        assert_eq!(
            RuleKind::try_from(kind),
            Err(ProtoConversionError::UnsupportedRuleKind { value })
        );
    }
}

#[test]
fn rule_ref_converts_kind_subject_and_revision() {
    let message = proto_rule_ref(Kind::Hold, proto_node_subject("team"), 7);
    assert_eq!(
        RuleRef::try_from(&message),
        Ok(RuleRef::new(RuleKind::Hold, node("team"), StreamRevision::new(7)))
    );
}

#[test]
fn rule_ref_keeps_a_missing_history_at_revision_zero() {
    let message = proto_rule_ref(Kind::ModelCeiling, proto_agent_subject("a"), 0);
    let converted = RuleRef::try_from(&message).expect("converts");
    assert!(converted.revision().is_missing());
}

#[test]
fn rule_ref_refuses_an_unknown_kind_a_missing_subject_and_a_bad_subject() {
    let mut unknown = proto_rule_ref(Kind::Hold, proto_agent_subject("a"), 1);
    unknown.kind = 99.into();
    assert_eq!(
        RuleRef::try_from(&unknown),
        Err(ProtoConversionError::UnknownEnumValue {
            enumeration: "PolicyRuleKind",
            value: 99,
        })
    );

    let unsupported = proto_rule_ref(Kind::AgentBudget, proto_agent_subject("a"), 1);
    assert_eq!(
        RuleRef::try_from(&unsupported),
        Err(ProtoConversionError::UnsupportedRuleKind { value: 1 })
    );

    let mut unset = proto_rule_ref(Kind::Hold, proto_agent_subject("a"), 1);
    unset.subject = Default::default();
    assert_eq!(
        RuleRef::try_from(&unset),
        Err(ProtoConversionError::MissingField {
            message: "PolicyRuleRef",
            field: "subject",
        })
    );

    let bad = proto_rule_ref(Kind::Hold, proto_agent_subject(""), 1);
    assert_eq!(
        RuleRef::try_from(&bad),
        Err(ProtoConversionError::InvalidAgentId(IdentifierError::Empty))
    );
}

// Egress

#[test]
fn egress_host_converts_to_a_destination() {
    assert_eq!(
        EgressDestination::try_from(&proto_host("api.example.com", 443)),
        Ok(destination("api.example.com", 443))
    );
}

#[test]
fn egress_host_refuses_a_pattern_and_a_bad_port() {
    assert_eq!(
        EgressDestination::try_from(&proto_host("*.example.com", 443)),
        Err(ProtoConversionError::InvalidHost(
            EgressHostNameError::InvalidCharacter { character: '*' }
        ))
    );
    assert_eq!(
        EgressDestination::try_from(&proto_host("example.com", 0)),
        Err(ProtoConversionError::InvalidPort(PortError::Zero))
    );
    assert_eq!(
        EgressDestination::try_from(&proto_host("example.com", 65_536)),
        Err(ProtoConversionError::InvalidPort(PortError::OutOfRange(65_536)))
    );
}

#[test]
fn egress_destination_converts_the_host_arm_and_refuses_an_unset_one() {
    assert_eq!(
        EgressDestination::try_from(&proto_destination("example.com", 80)),
        Ok(destination("example.com", 80))
    );
    assert_eq!(
        EgressDestination::try_from(&egress::EgressDestination::default()),
        Err(ProtoConversionError::MissingField {
            message: "EgressDestination",
            field: "destination",
        })
    );
}

#[test]
fn egress_permission_converts_and_collapses_a_repeated_destination() {
    let message = proto_egress_policy(&[("a.example.com", 443), ("b.example.com", 80), ("a.example.com", 443)]);
    assert_eq!(
        EgressPermission::try_from(&message),
        Ok(egress_permission(&[("a.example.com", 443), ("b.example.com", 80)]))
    );
}

#[test]
fn egress_permission_refuses_an_empty_policy_and_any_bad_destination() {
    assert_eq!(
        EgressPermission::try_from(&egress::EgressPermissionPolicy::default()),
        Err(ProtoConversionError::EmptyPolicy {
            policy: "EgressPermissionPolicy",
        })
    );
    let one_bad = proto_egress_policy(&[("ok.example.com", 443), ("Bad.example.com", 443)]);
    assert_eq!(
        EgressPermission::try_from(&one_bad),
        Err(ProtoConversionError::InvalidHost(EgressHostNameError::Uppercase))
    );
}

// Delegation

#[test]
fn delegation_target_converts_both_arms() {
    assert_eq!(
        DelegationTarget::try_from(&proto_target_agent("b")),
        Ok(DelegationTarget::Agent(agent_id("b")))
    );
    assert_eq!(
        DelegationTarget::try_from(&proto_target_under("team")),
        Ok(DelegationTarget::AgentsUnder(node_id("team")))
    );
}

#[test]
fn delegation_target_refuses_an_unset_arm_and_an_invalid_identity() {
    assert_eq!(
        DelegationTarget::try_from(&delegation::DelegationTarget::default()),
        Err(ProtoConversionError::MissingField {
            message: "DelegationTarget",
            field: "target",
        })
    );
    assert_eq!(
        DelegationTarget::try_from(&proto_target_agent("")),
        Err(ProtoConversionError::InvalidAgentId(IdentifierError::Empty))
    );
    assert_eq!(
        DelegationTarget::try_from(&proto_target_under("")),
        Err(ProtoConversionError::InvalidNodeId(IdentifierError::Empty))
    );
}

#[test]
fn delegation_permission_converts_and_collapses_a_repeated_target() {
    let message = delegation::DelegationPermissionPolicy {
        permitted: vec![
            proto_target_agent("b"),
            proto_target_under("team"),
            proto_target_agent("b"),
        ],
    };
    assert_eq!(
        DelegationPermission::try_from(&message),
        Ok(delegation_permission(vec![
            DelegationTarget::Agent(agent_id("b")),
            DelegationTarget::AgentsUnder(node_id("team")),
        ]))
    );
}

#[test]
fn delegation_permission_refuses_an_empty_policy_and_any_bad_target() {
    assert_eq!(
        DelegationPermission::try_from(&delegation::DelegationPermissionPolicy::default()),
        Err(ProtoConversionError::EmptyPolicy {
            policy: "DelegationPermissionPolicy",
        })
    );
    let one_bad = delegation::DelegationPermissionPolicy {
        permitted: vec![proto_target_agent("b"), delegation::DelegationTarget::default()],
    };
    assert!(matches!(
        DelegationPermission::try_from(&one_bad),
        Err(ProtoConversionError::MissingField { .. })
    ));
}

// Model ceiling

#[test]
fn model_id_converts_and_refuses_an_unset_or_invalid_matcher() {
    assert_eq!(
        ModelId::try_from(&proto_matcher("claude-x")),
        Ok(ModelId::new("claude-x").unwrap())
    );
    assert_eq!(
        ModelId::try_from(&ceilings::ModelMatcher::default()),
        Err(ProtoConversionError::MissingField {
            message: "ModelMatcher",
            field: "matcher",
        })
    );
    assert_eq!(
        ModelId::try_from(&proto_matcher("")),
        Err(ProtoConversionError::InvalidModelId(IdentifierError::Empty))
    );
}

#[test]
fn model_ceiling_converts_and_collapses_a_repeated_model() {
    let message = ceilings::ModelCeilingPolicy {
        within: vec![proto_matcher("m1"), proto_matcher("m2"), proto_matcher("m1")],
    };
    assert_eq!(ModelCeiling::try_from(&message), Ok(model_ceiling(&["m1", "m2"])));
}

#[test]
fn model_ceiling_refuses_an_empty_policy_rather_than_reading_it_as_admit_nothing() {
    assert_eq!(
        ModelCeiling::try_from(&ceilings::ModelCeilingPolicy::default()),
        Err(ProtoConversionError::EmptyPolicy {
            policy: "ModelCeilingPolicy",
        })
    );
}

#[test]
fn model_ceiling_refuses_any_bad_matcher() {
    let message = ceilings::ModelCeilingPolicy {
        within: vec![proto_matcher("m1"), proto_matcher("")],
    };
    assert_eq!(
        ModelCeiling::try_from(&message),
        Err(ProtoConversionError::InvalidModelId(IdentifierError::Empty))
    );
}

// Holds

#[test]
fn hold_name_converts_and_refuses_an_invalid_name() {
    let converted = HoldName::try_from(&holds::HoldName {
        value: "security-review".to_owned(),
    });
    assert_eq!(converted, Ok(HoldName::new("security-review").unwrap()));
    assert_eq!(
        HoldName::try_from(&holds::HoldName::default()),
        Err(ProtoConversionError::InvalidHoldName(IdentifierError::Empty))
    );
}

#[test]
fn hold_scope_converts_both_scopes_and_refuses_unspecified() {
    assert_eq!(HoldScope::try_from(holds::HoldScope::NewWork), Ok(HoldScope::NewWork));
    assert_eq!(
        HoldScope::try_from(holds::HoldScope::AllProtectedActions),
        Ok(HoldScope::AllProtectedActions)
    );
    assert_eq!(
        HoldScope::try_from(holds::HoldScope::Unspecified),
        Err(ProtoConversionError::UnknownEnumValue {
            enumeration: "HoldScope",
            value: 0,
        })
    );
}

#[test]
fn hold_converts_name_and_scope() {
    assert_eq!(
        Hold::try_from(&proto_hold("oncall", holds::HoldScope::AllProtectedActions)),
        Ok(hold("oncall", HoldScope::AllProtectedActions))
    );
}

#[test]
fn hold_refuses_a_missing_name_an_unknown_scope_and_a_bad_name() {
    let mut unnamed = proto_hold("oncall", holds::HoldScope::NewWork);
    unnamed.name = Default::default();
    assert_eq!(
        Hold::try_from(&unnamed),
        Err(ProtoConversionError::MissingField {
            message: "Hold",
            field: "name",
        })
    );

    let mut unknown = proto_hold("oncall", holds::HoldScope::NewWork);
    unknown.scope = 42.into();
    assert_eq!(
        Hold::try_from(&unknown),
        Err(ProtoConversionError::UnknownEnumValue {
            enumeration: "HoldScope",
            value: 42,
        })
    );

    assert_eq!(
        Hold::try_from(&proto_hold("", holds::HoldScope::NewWork)),
        Err(ProtoConversionError::InvalidHoldName(IdentifierError::Empty))
    );
}

// Snapshot

#[test]
fn converted_messages_fill_a_snapshot_the_evaluator_reads() {
    let permission = EgressPermission::try_from(&proto_egress_policy(&[("api.example.com", 443)])).unwrap();
    let team = Subject::try_from(&proto_node_subject("team")).unwrap();
    let snapshot = PolicySnapshot::builder(stamped_at())
        .egress_permission(team, 3, permission)
        .build();
    let policy_view = view(snapshot);
    let evaluator = enforce();

    let allowed = evaluator.evaluate(
        &policy_view,
        &egress_request(chain_of_a(), "api.example.com", 443),
        &[],
        fresh_now(),
    );
    let denied = evaluator.evaluate(
        &policy_view,
        &egress_request(chain_of_a(), "other.example.com", 443),
        &[],
        fresh_now(),
    );
    assert_eq!(allowed.effective(), &Effective::Admit);
    assert!(matches!(denied.effective(), Effective::Refuse(_)));
}
