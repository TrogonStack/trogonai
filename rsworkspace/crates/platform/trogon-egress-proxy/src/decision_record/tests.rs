use std::path::PathBuf;

use buffa::EnumValue;
use trogon_audit::{Obligation, Stamper};
use trogon_std::time::SystemClock;
use trogon_std::uuid::UuidV7Generator;
use trogonai_proto::platform::audit::v1::audit_record::Detail;
use trogonai_proto::platform::audit::v1::{
    AuditClass, AuditDecision, EnforcementMode, GatewayDecision, GatewaySurface, TlsMode as ProtoTlsMode,
};

use super::*;
use crate::agent_id::AgentId;
use crate::fault::Fault;
use crate::policy_source::PolicySource;

fn source() -> PolicySource {
    PolicySource::StaticFile {
        path: PathBuf::from("policy.toml"),
    }
}

fn record_for(destination: Option<&Destination>, decision: Decision) -> Record {
    let agent = AgentId::new("agent-1").expect("agent");
    let source = source();
    let event = AuditEvent {
        agent: &agent,
        destination,
        resolved_address: Some("93.184.216.34".parse().expect("ip")),
        decision,
        snapshot_source: &source,
    };
    let stamper = Stamper::new(SystemClock, UuidV7Generator);
    decision_record(
        &event,
        &AuditName::new("acme").expect("tenant"),
        &NatsToken::new("payments").expect("project"),
        stamper.stamp(),
    )
    .expect("record")
}

fn gateway(record: &Record) -> &GatewayDecision {
    match record.message().detail.as_ref().expect("detail") {
        Detail::GatewayDecision(decision) => decision,
        other => panic!("not a decision: {other:?}"),
    }
}

fn destination(text: &str) -> Destination {
    text.parse().expect("destination")
}

#[test]
fn a_permit_is_an_enforced_allow_that_must_be_recorded() {
    let destination = destination("api.example.com:443");
    let record = record_for(Some(&destination), Decision::Permitted);
    let message = record.message();
    assert_eq!(record.obligation(), Obligation::BeforeEffect);
    assert_eq!(message.class, EnumValue::from(AuditClass::Must));
    assert_eq!(message.tenant.value, "acme");
    assert_eq!(message.project.value, "payments");
    assert_eq!(
        message.attribution.decision,
        EnumValue::from(AuditDecision::AUDIT_DECISION_ALLOW)
    );
    assert_eq!(
        message.attribution.enforcement_mode,
        EnumValue::from(EnforcementMode::ENFORCEMENT_MODE_ENFORCE)
    );
    assert_eq!(message.attribution.reason_code, None);
    assert_eq!(message.attribution.policy_source.as_deref(), Some("static_file"));
    assert!(message.attribution.rule_ids.is_empty());
    assert_eq!(message.attribution.projection_revision, None);

    let decision = gateway(&record);
    assert_eq!(decision.target, "api.example.com:443");
    assert_eq!(
        decision.surface,
        EnumValue::from(GatewaySurface::GATEWAY_SURFACE_EGRESS)
    );
    assert_eq!(
        decision.tls_mode,
        Some(EnumValue::from(ProtoTlsMode::TLS_MODE_PASSTHROUGH))
    );
}

#[test]
fn the_calling_agent_is_the_actor() {
    let destination = destination("api.example.com:443");
    let record = record_for(Some(&destination), Decision::Permitted);
    let actor = format!("{:?}", record.message().actor);
    assert!(actor.contains("agent-1"), "{actor}");
}

#[test]
fn a_refusal_is_a_deny_with_the_fault_as_its_lower_snake_reason() {
    let destination = destination("evil.example:443");
    let record = record_for(Some(&destination), Decision::Refused(Fault::NoPermit));
    let attribution = &record.message().attribution;
    assert_eq!(
        attribution.decision,
        EnumValue::from(AuditDecision::AUDIT_DECISION_DENY)
    );
    assert_eq!(attribution.reason_code.as_deref(), Some("no_permit"));
    assert_eq!(record.obligation(), Obligation::Stands);
}

#[test]
fn the_resolved_address_has_no_field_in_the_record() {
    let destination = destination("api.example.com:443");
    let record = record_for(Some(&destination), Decision::Permitted);
    assert!(!format!("{:?}", record.message()).contains("93.184.216.34"));
}

#[test]
fn a_request_without_a_destination_is_recorded_under_a_sentinel_target() {
    let record = record_for(None, Decision::Refused(Fault::BadRequest));
    assert_eq!(gateway(&record).target, NO_DESTINATION_TARGET);
}

#[test]
fn a_destination_too_long_for_an_audit_name_is_recorded_under_a_sentinel_target() {
    let label = "a".repeat(63);
    let host = format!("{label}.{label}.{label}.{}", "a".repeat(61));
    let destination = destination(&format!("{host}:443"));
    let record = record_for(Some(&destination), Decision::Refused(Fault::NoPermit));
    assert_eq!(gateway(&record).target, DESTINATION_TOO_LONG_TARGET);
}

#[test]
fn every_fault_has_a_reason_code_the_record_accepts() {
    for fault in [
        Fault::NoPermit,
        Fault::AddressForbidden,
        Fault::SniMismatch,
        Fault::PolicyUnavailable,
        Fault::UpstreamUnreachable,
        Fault::MethodNotAllowed,
        Fault::BadRequest,
    ] {
        let record = record_for(None, Decision::Refused(fault));
        assert_eq!(
            record.message().attribution.reason_code.as_deref(),
            Some(fault.reason_code())
        );
    }
}
