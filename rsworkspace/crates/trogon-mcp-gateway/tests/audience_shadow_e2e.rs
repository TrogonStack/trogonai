//! End-to-end shadow audience mismatch path via [`RecordingAuditSink`].

use trogon_mcp_gateway::audience_shadow::{
    AudienceCheckContext, AudienceShadowChecker, AudienceShadowMode, AudienceShadowOutcome, ClaimAud,
    RecordingAuditSink, AUD_MISMATCH_AUDIT_SUBJECT, compute_expected_aud,
};

#[test]
fn shadow_mismatch_emits_aud_mismatch_envelope() {
    let sink = RecordingAuditSink::new();
    let ctx = AudienceCheckContext {
        tenant_id: "acme".into(),
        caller_sub: "agent:acme/oncall".into(),
        request_id: "jsonrpc-req-42".into(),
    };
    let expected = compute_expected_aud("acme", "gw-prod-1").expect("valid segments");
    let presented = "urn:trogon:mcp:gateway:acme:gw-prod-1";

    let outcome = AudienceShadowChecker::check(
        Some(&sink),
        &ctx,
        ClaimAud::Single(presented),
        "acme",
        "gw-prod-1",
        AudienceShadowMode::Shadow,
    )
    .expect("shadow mismatch is not a validation error");

    assert_eq!(outcome, AudienceShadowOutcome::ShadowMismatch);

    let records = sink.drain();
    assert_eq!(records.len(), 1);
    let envelope = &records[0];
    assert_eq!(envelope.tenant_id, "acme");
    assert_eq!(envelope.caller_sub, "agent:acme/oncall");
    assert_eq!(envelope.request_id, "jsonrpc-req-42");
    assert_eq!(envelope.expected_aud, expected);
    assert_eq!(envelope.observed_aud, vec![presented.to_string()]);
    assert!(envelope.ts_unix_ms > 0);

    // Subject is fixed for JetStream wiring in a follow-up PR.
    assert_eq!(AUD_MISMATCH_AUDIT_SUBJECT, "mcp.audit.gateway.aud_mismatch");
}
