use std::time::Duration;

use trogonai_proto::platform::audit::v1::audit_record::Detail;
use trogonai_proto::platform::audit::v1::{AuditClass, AuditDecision, EnforcementMode};

use super::*;
use crate::consistency_token::ConsistencyToken;
use crate::fixtures::{change, credential, decision_facts, deny, name, scope, scope_for, stamper};
use crate::read_only::ReadOnly;
use crate::reason_code::ReasonCode;
use crate::trace_id::TraceId;

fn gateway(record: &Record) -> &GatewayDecision {
    match record.message().detail.as_ref().unwrap() {
        Detail::GatewayDecision(decision) => decision,
        other => panic!("not a decision: {other:?}"),
    }
}

#[test]
fn a_decision_carries_its_class_subject_and_attribution() {
    let stamper = stamper();
    let stamp = stamper.stamp();
    let id = stamp.id();
    let record = Record::decision(
        scope(),
        decision_facts(DecisionOutcome::Deny, ReadOnly::Undeclared),
        stamp,
    );

    assert_eq!(record.id(), id);
    assert_eq!(record.subject().render(), "platform.v1.audit.decision.project_1.deny");
    assert_eq!(record.obligation(), Obligation::Stands);
    assert!(matches!(record.kind(), RecordKind::Decision { .. }));

    let message = record.message();
    assert_eq!(message.record_id, id.to_string());
    assert_eq!(message.class, EnumValue::from(AuditClass::Must));
    assert_eq!(message.tenant.value, "tenant_1");
    assert_eq!(message.project.value, "project_1");
    assert_eq!(
        message.attribution.decision,
        EnumValue::from(AuditDecision::AUDIT_DECISION_DENY)
    );
    assert_eq!(
        message.attribution.enforcement_mode,
        EnumValue::from(EnforcementMode::ENFORCEMENT_MODE_ENFORCE)
    );
    assert!(!message.attribution.would_deny);
    assert_eq!(gateway(&record).target, "api.example.com");
}

#[test]
fn a_would_deny_is_an_allow_in_dry_run_flagged_would_deny() {
    let stamper = stamper();
    let record = Record::decision(
        scope(),
        decision_facts(DecisionOutcome::WouldDeny, ReadOnly::Undeclared),
        stamper.stamp(),
    );
    let attribution = &record.message().attribution;
    assert_eq!(
        attribution.decision,
        EnumValue::from(AuditDecision::AUDIT_DECISION_ALLOW)
    );
    assert_eq!(
        attribution.enforcement_mode,
        EnumValue::from(EnforcementMode::ENFORCEMENT_MODE_DRY_RUN)
    );
    assert!(attribution.would_deny);
    assert_eq!(record.message().class, EnumValue::from(AuditClass::BestEffort));
    assert_eq!(
        record.subject().render(),
        "platform.v1.audit.decision.project_1.would_deny"
    );
}

#[test]
fn a_break_glass_actor_upgrades_a_read_only_allow() {
    let stamper = stamper();
    let facts = || decision_facts(DecisionOutcome::Allow(Enforcement::Enforce), ReadOnly::Declared);
    let ordinary = Record::decision(scope(), facts(), stamper.stamp());
    assert_eq!(ordinary.message().class, EnumValue::from(AuditClass::BestEffort));
    let glass = scope_for(Actor::BreakGlass {
        operator: name("op"),
        host: name("host-1"),
    });
    let upgraded = Record::decision(glass, facts(), stamper.stamp());
    assert_eq!(upgraded.message().class, EnumValue::from(AuditClass::Must));
}

#[test]
fn every_optional_fact_reaches_the_message() {
    let stamper = stamper();
    let mut layer = LayerFacts::new(name("policy"), DecisionOutcome::Deny);
    layer.reason = Some(ReasonCode::new("no_rule").unwrap());
    layer.rule_ids = vec![name("rule-1")];
    layer.consistency_token = Some(ConsistencyToken::new("CAESAggB+/w=").unwrap());
    let mut dry = LayerFacts::new(name("tenancy"), DecisionOutcome::WouldDeny);
    dry.reason = None;

    let mut facts = decision_facts(DecisionOutcome::Deny, ReadOnly::Undeclared);
    facts.reason = Some(ReasonCode::new("no_rule").unwrap());
    facts.rule_ids = vec![name("rule-1"), name("rule-2")];
    facts.policy_source = Some(name("policy_source_1"));
    facts.projection_revision = Some(12);
    facts.request_bytes = Some(10);
    facts.response_bytes = Some(20);
    facts.latency = Some(Duration::from_millis(1_500));
    facts.tls_mode = Some(TlsMode::Intercepted);
    facts.session = Some(name("session-1"));
    facts.layers = vec![layer, dry];
    facts.surface = Surface::A2aIngress;

    let scope = scope()
        .with_act_chain(vec![name("agent-a"), name("agent-b")])
        .unwrap()
        .with_trace_id(TraceId::new("4bf92f3577b34da6a3ce929d0e0e4736").unwrap())
        .with_request_id(name("req-1"));
    let record = Record::decision(scope, facts, stamper.stamp());
    let message = record.message();

    assert_eq!(message.attribution.reason_code.as_deref(), Some("no_rule"));
    assert_eq!(
        message.attribution.rule_ids,
        vec!["rule-1".to_owned(), "rule-2".to_owned()]
    );
    assert_eq!(message.attribution.policy_source.as_deref(), Some("policy_source_1"));
    assert_eq!(message.attribution.projection_revision, Some(12));
    assert_eq!(message.act_chain.len(), 2);
    assert_eq!(message.act_chain[1].agent_id.value, "agent-b");
    assert_eq!(message.trace_id.as_deref(), Some("4bf92f3577b34da6a3ce929d0e0e4736"));
    assert_eq!(message.request_id.as_deref(), Some("req-1"));

    let decision = gateway(&record);
    assert_eq!(
        decision.surface,
        EnumValue::from(GatewaySurface::GATEWAY_SURFACE_A2A_INGRESS)
    );
    assert_eq!(decision.request_bytes, Some(10));
    assert_eq!(decision.response_bytes, Some(20));
    assert_eq!(decision.latency.seconds, 1);
    assert_eq!(decision.latency.nanos, 500_000_000);
    assert_eq!(
        decision.tls_mode,
        Some(EnumValue::from(ProtoTlsMode::TLS_MODE_INTERCEPTED))
    );
    assert_eq!(decision.session.value, "session-1");
    assert_eq!(decision.layer_verdicts.len(), 2);
    let first = &decision.layer_verdicts[0];
    assert_eq!(first.layer, "policy");
    assert_eq!(first.reason_code.as_deref(), Some("no_rule"));
    assert_eq!(first.rule_ids, vec!["rule-1".to_owned()]);
    assert_eq!(first.consistency_token.as_deref(), Some("CAESAggB+/w="));
    assert!(!first.would_deny);
    let second = &decision.layer_verdicts[1];
    assert!(second.would_deny);
    assert_eq!(second.reason_code, None);
}

#[test]
fn passthrough_and_egress_map_to_their_proto_values() {
    let stamper = stamper();
    let mut facts = decision_facts(DecisionOutcome::Allow(Enforcement::Enforce), ReadOnly::Undeclared);
    facts.tls_mode = Some(TlsMode::Passthrough);
    let record = Record::decision(scope(), facts, stamper.stamp());
    let decision = gateway(&record);
    assert_eq!(
        decision.surface,
        EnumValue::from(GatewaySurface::GATEWAY_SURFACE_EGRESS)
    );
    assert_eq!(
        decision.tls_mode,
        Some(EnumValue::from(ProtoTlsMode::TLS_MODE_PASSTHROUGH))
    );
    assert!(decision.session.is_unset());
    assert!(decision.latency.is_unset());
}

#[test]
fn every_actor_kind_maps_to_its_message() {
    let stamper = stamper();
    let actors = [
        Actor::Unattributed,
        Actor::ApiKey {
            api_key_id: name("key-1"),
        },
        Actor::HumanSession {
            subject: name("user-1"),
            issuer: name("issuer-1"),
        },
        Actor::BreakGlass {
            operator: name("op"),
            host: name("host-1"),
        },
        Actor::Agent {
            agent_id: name("agent-7"),
        },
        Actor::Service {
            service_id: name("billing-runner"),
        },
    ];
    let mut kinds = Vec::new();
    for actor in actors {
        let record = Record::decision(
            scope_for(actor),
            decision_facts(DecisionOutcome::Deny, ReadOnly::Undeclared),
            stamper.stamp(),
        );
        kinds.push(format!("{:?}", record.message().actor.kind));
    }
    kinds.dedup();
    assert_eq!(kinds.len(), 6);
    assert!(kinds[1].contains("key-1"));
    assert!(kinds[2].contains("issuer-1"));
    assert!(kinds[3].contains("host-1"));
    assert!(kinds[4].contains("agent-7"));
    assert!(kinds[5].contains("billing-runner"));
}

#[test]
fn a_service_is_named_as_a_service_and_never_as_an_agent() {
    let stamper = stamper();
    let record = Record::decision(
        scope_for(Actor::Service {
            service_id: name("billing-runner"),
        }),
        decision_facts(DecisionOutcome::Deny, ReadOnly::Undeclared),
        stamper.stamp(),
    );
    assert_eq!(
        record.message().actor,
        MessageField::some(AuditActor {
            kind: Some(
                ServiceActor {
                    service_id: "billing-runner".to_owned(),
                }
                .into()
            ),
        })
    );
}

#[test]
fn a_secrets_release_decision_maps_to_its_surface() {
    let stamper = stamper();
    let mut facts = decision_facts(DecisionOutcome::Deny, ReadOnly::Undeclared);
    facts.surface = Surface::SecretsRelease;
    let record = Record::decision(scope(), facts, stamper.stamp());
    assert_eq!(
        gateway(&record).surface,
        EnumValue::from(GatewaySurface::GATEWAY_SURFACE_SECRETS_RELEASE)
    );
}

#[test]
fn a_credential_use_is_must_before_the_effect_with_an_allow_attribution() {
    let stamper = stamper();
    let record = credential(&stamper);
    assert_eq!(
        record.subject().render(),
        "platform.v1.audit.credential.project_1.inject"
    );
    assert_eq!(record.obligation(), Obligation::BeforeEffect);
    assert_eq!(record.message().class, EnumValue::from(AuditClass::Must));
    assert_eq!(
        record.message().attribution.decision,
        EnumValue::from(AuditDecision::AUDIT_DECISION_ALLOW)
    );
    let Some(Detail::CredentialUse(usage)) = record.message().detail.as_ref() else {
        panic!("not a credential use");
    };
    assert_eq!(usage.credential, "github_token");
    assert_eq!(usage.binding_id, "binding_1");
    assert_eq!(
        usage.kind,
        EnumValue::from(CredentialUseKind::CREDENTIAL_USE_KIND_INJECT)
    );
    assert_eq!(usage.destination, None);
    assert_eq!(usage.decision_record_id, None);
    assert_eq!(usage.location, None);
}

#[test]
fn a_release_links_to_its_decision_and_destination() {
    let stamper = stamper();
    let decision = deny(&stamper);
    let mut facts = CredentialFacts::new(CredentialTerminal::Release, name("api_key"), name("binding_2"));
    facts.destination = Some(name("api.example.com"));
    facts.decision_record_id = Some(decision.id());
    facts.location = Some(name("header:authorization"));
    let record = Record::credential_use(scope(), facts, stamper.stamp());
    assert_eq!(
        record.subject().render(),
        "platform.v1.audit.credential.project_1.release"
    );
    let Some(Detail::CredentialUse(usage)) = record.message().detail.as_ref() else {
        panic!("not a credential use");
    };
    assert_eq!(
        usage.kind,
        EnumValue::from(CredentialUseKind::CREDENTIAL_USE_KIND_RELEASE)
    );
    assert_eq!(usage.destination.as_deref(), Some("api.example.com"));
    assert_eq!(usage.decision_record_id, Some(decision.id().to_string()));
    assert_eq!(usage.location.as_deref(), Some("header:authorization"));
}

#[test]
fn a_change_is_must_with_no_attribution() {
    let stamper = stamper();
    let record = change(&stamper);
    assert_eq!(record.subject().render(), "platform.v1.audit.change.project_1.policy");
    assert_eq!(record.obligation(), Obligation::BeforeEffect);
    assert!(record.message().attribution.is_unset());
    let Some(Detail::Change(change)) = record.message().detail.as_ref() else {
        panic!("not a change");
    };
    assert_eq!(change.kind, EnumValue::from(ChangeKind::CHANGE_KIND_POLICY));
    assert_eq!(change.resource_name, "policies/egress");
    assert_eq!(change.stream_sequence, 42);
    assert_eq!(change.event_id, "evt_1");
}

#[test]
fn a_credential_change_maps_to_its_kind() {
    let stamper = stamper();
    let facts = ChangeFacts {
        kind: ChangeTerminal::Credential,
        resource_name: name("credentials/api"),
        event_type: name("rotated"),
        source_stream: name("CRED_EVENTS"),
        stream_sequence: 1,
        event_id: name("evt_2"),
    };
    let record = Record::change(scope(), facts, stamper.stamp());
    assert_eq!(
        record.subject().render(),
        "platform.v1.audit.change.project_1.credential"
    );
    let Some(Detail::Change(change)) = record.message().detail.as_ref() else {
        panic!("not a change");
    };
    assert_eq!(change.kind, EnumValue::from(ChangeKind::CHANGE_KIND_CREDENTIAL));
}

#[test]
fn a_backdated_stamp_sets_the_occurrence_time() {
    let stamper = stamper();
    let earlier = std::time::UNIX_EPOCH + Duration::from_secs(60);
    let stamp = stamper.stamp().occurring_at(earlier);
    let record = Record::decision(
        scope(),
        decision_facts(DecisionOutcome::Deny, ReadOnly::Undeclared),
        stamp,
    );
    assert_eq!(record.message().occurred_at.seconds, 60);
    assert_ne!(record.message().recorded_at.seconds, 60);
}

#[test]
fn a_chain_reset_opens_a_chain_as_a_must_record_of_no_one() {
    let identity = WriterIdentity::new(
        crate::fixtures::writer_id(),
        crate::fixtures::token("egress_proxy"),
        name("tenant_1"),
        crate::fixtures::token("project_1"),
    );
    let stamp = stamper().stamp();
    let record = Record::chain_reset(&identity, stamp);

    assert_eq!(record.id(), stamp.id());
    assert_eq!(record.kind(), RecordKind::Integrity);
    assert_eq!(record.obligation(), Obligation::BeforeEffect);
    assert_eq!(
        record.subject().render(),
        "platform.v1.audit.integrity.egress_proxy.chain_reset"
    );
    let message = record.message();
    assert_eq!(message.class, EnumValue::from(AuditClass::Must));
    assert_eq!(message.tenant.value, "tenant_1");
    assert_eq!(message.project.value, "project_1");
    assert_eq!(message.actor, MessageField::some(audit_actor(&Actor::Unattributed)));
    assert!(message.attribution.as_option().is_none());
    let Some(Detail::WriterLifecycle(lifecycle)) = message.detail.as_ref() else {
        panic!("not a lifecycle record: {:?}", message.detail);
    };
    assert_eq!(lifecycle.kind, EnumValue::from(WriterLifecycleKind::ChainReset));
    assert_eq!(
        lifecycle.reset_reason,
        Some(EnumValue::from(ChainResetReason::ProcessStartWithoutResume))
    );
}
