mod aauth_tests;
mod fixture;
mod pressure_tests;
mod signed_auth_tests;
mod streaming_tests;

use std::sync::Arc;

use a2a_nats::constants::GATEWAY_CALLER_ID_HEADER;
use a2a_redaction::{SkillId, WasmBundlePath};
use serde_json::{Value, json};

use crate::constants::{ENV_GATEWAY_JWT_AUDIENCE, ENV_GATEWAY_TRUST_CALLER_HEADERS, ENV_TIER3_REDACTION_ENABLED};
use crate::gateway_test_support::Diagnostics;
use crate::policy::spicedb_tier1::Tier1AuthorizeOutcome;
use crate::policy::tier1_declarative::RealTier1DeclarativeGate;
use crate::policy::tier1_declarative::bundle::{
    Tier1DeclarativeBundle, Tier1DeclarativeEffect, Tier1DeclarativeMatch, Tier1DeclarativeRule,
    Tier1DeclarativeRuleId, Tier1ResourceKind,
};
use crate::policy::tier2::{DenyAllTier2Evaluator, NoopTier2Evaluator};
use crate::policy::tier3_redaction::{
    RedactionRewrite, RewriteKind, Tier3EngineError, Tier3EvaluationContext, Tier3RedactionDecision,
    Tier3RedactionGate, Tier3RefusalReason,
};
use crate::policy::wasmtime_substrate::{Tier2State, WasmtimeSubstrate};

use fixture::{DispatchFixture, RecordingOwner, RecordingTier1, TestResult, assert_empty, receive, request};

#[tokio::test]
async fn allowed_request_preserves_payload_headers_and_reply_route() -> TestResult {
    let mut fixture = DispatchFixture::new().await?;
    let mut message = request("message.send", json!({"message": {"role": "user", "parts": []}}));
    message
        .headers
        .as_mut()
        .expect("headers")
        .insert("X-Correlation", "trace-1");
    let expected: Value = serde_json::from_slice(&message.payload)?;
    let expected_headers = message.headers.clone();
    fixture.dispatch(message).await;

    let forwarded = receive(&mut fixture.agents).await?;
    assert_eq!(forwarded.subject.as_str(), "a2a.v1.agents.bot.message.send");
    assert_eq!(
        forwarded.reply.as_ref().map(async_nats::Subject::as_str),
        Some("_INBOX.dispatch")
    );
    assert_eq!(forwarded.headers, expected_headers);
    assert_eq!(serde_json::from_slice::<Value>(&forwarded.payload)?, expected);
    let audit = fixture.audit("ok").await?;
    assert_eq!(audit["req_id"], "request-7");
    assert_eq!(audit["method"], "message/send");
    assert_eq!(audit["caller_id"], "alice");
    assert_eq!(audit["caller_source"], "header_trusted");
    assert_eq!(
        audit["rules_fired"],
        json!([
            "gateway.tier1.layer_disabled",
            "gateway.tier2.layer_disabled",
            "gateway.tier3.layer_disabled",
            "gateway.aauth.layer_disabled",
        ])
    );
    assert_eq!(
        audit["rewrites"],
        json!(["ingress:a2a.v1.gateway.bot.message.send -> agent:a2a.v1.agents.bot.message.send",])
    );

    fixture
        .client
        .publish(
            forwarded.reply.expect("agent reply inbox"),
            br#"{"jsonrpc":"2.0","id":"request-7","result":{}}"#.as_slice().into(),
        )
        .await?;
    let reply = receive(&mut fixture.replies).await?;
    assert_eq!(serde_json::from_slice::<Value>(&reply.payload)?["result"], json!({}));
    Ok(())
}

#[tokio::test]
async fn untrusted_caller_headers_cannot_change_audit_attribution() -> TestResult {
    let mut fixture = DispatchFixture::new().await?;
    fixture.env.set(ENV_GATEWAY_TRUST_CALLER_HEADERS, "false");
    fixture.dispatch(request("tasks.get", json!({"id": "task-1"}))).await;
    assert_eq!(
        receive(&mut fixture.agents).await?.subject.as_str(),
        "a2a.v1.agents.bot.tasks.get"
    );
    let audit = fixture.audit("ok").await?;
    assert_eq!(audit["caller_id"], "_");
    assert!(audit.get("caller_source").is_none());
    Ok(())
}

#[tokio::test]
async fn requests_without_reply_inbox_are_not_dispatched_or_audited() -> TestResult {
    let diagnostics = Diagnostics::both_outputs();
    let mut fixture = DispatchFixture::new().await?;
    let mut message = request("message.send", json!({}));
    message.reply = None;
    let payload_len = message.payload.len().to_string();
    fixture.dispatch(message).await;
    diagnostics.assert_event("gateway ingress envelope received", &[("payload_len", &payload_len)]);
    assert_empty(&fixture.client, &mut fixture.agents, "a2a.v1.agents.barrier").await?;
    assert_empty(&fixture.client, &mut fixture.replies, "_INBOX.dispatch").await?;
    assert_empty(&fixture.client, &mut fixture.audits, "a2a.v1.audit.barrier").await
}

#[tokio::test]
async fn invalid_route_replies_with_correlated_invalid_request() -> TestResult {
    let diagnostics = Diagnostics::both_outputs();
    let mut fixture = DispatchFixture::new().await?;
    for subject in ["other.v1.gateway.bot.message.send", "a2a.v1.gateway.bot.unknown"] {
        let mut message = request("message.send", json!({}));
        message.subject = subject.into();
        fixture.dispatch(message).await;
        fixture.denied(-32600, json!("request-7")).await?;
    }
    diagnostics.assert_event(
        "gateway ingress subject routing failed",
        &[("ingress.subject", "a2a.v1.gateway.bot.unknown")],
    );
    Ok(())
}

#[tokio::test]
async fn malformed_json_is_denied_before_agent_publish() -> TestResult {
    let mut fixture = DispatchFixture::new().await?;
    let mut message = request("message.send", json!({}));
    message.payload = bytes::Bytes::from_static(b"{");
    fixture.dispatch(message).await;
    fixture.denied(-32801, Value::Null).await?;
    Ok(())
}

#[tokio::test]
async fn tier1_authorizes_the_routed_agent_and_emits_owner() -> TestResult {
    let mut fixture = DispatchFixture::new().await?;
    let gate = Arc::new(RecordingTier1::new(Tier1AuthorizeOutcome::Allowed {
        zed_token: Some("zed-1".into()),
    }));
    let owner = Arc::new(RecordingOwner::default());
    fixture.tier1 = gate.clone();
    fixture.owner = Some(owner.clone());
    fixture.env.set(ENV_GATEWAY_JWT_AUDIENCE, "tenant-acme");
    fixture.dispatch(request("message.send", json!({"id": "task-1"}))).await;
    receive(&mut fixture.agents).await?;
    let calls = gate.calls.lock().expect("authorization calls").clone();
    assert_eq!(calls.len(), 1);
    let call = &calls[0];
    assert_eq!(call.session.account(), "tenant-acme");
    assert_eq!(call.session.sub(), "alice");
    assert_eq!(
        call.principal.spicedb_subject().expect("principal").as_str(),
        "user/alice"
    );
    assert_eq!(call.tuple.resource_type.as_str(), "agent");
    assert_eq!(call.tuple.resource_id.as_str(), "bot");
    assert_eq!(call.tuple.permission.as_str(), "invoke");
    let owner_calls = owner.calls.lock().expect("owner calls").clone();
    assert_eq!(owner_calls.len(), 1);
    assert_eq!(owner_calls[0].resource_id.as_str(), "bot:task-1");
    assert_eq!(owner_calls[0].relation, "owner");
    assert_eq!(owner_calls[0].subject_type, "user");
    assert_eq!(owner_calls[0].subject_id, "alice");
    let audit = fixture.audit("ok").await?;
    assert_eq!(audit["tier1_decision"], "allow");
    assert_eq!(audit["zed_token_snapshot"], "zed-1");
    Ok(())
}

#[tokio::test]
async fn owner_write_failure_preserves_authorized_forwarding() -> TestResult {
    let mut fixture = DispatchFixture::new().await?;
    fixture.tier1 = Arc::new(RecordingTier1::new(Tier1AuthorizeOutcome::Allowed { zed_token: None }));
    let owner = Arc::new(RecordingOwner {
        failure: Some(tonic::Status::unavailable("test outage")),
        ..Default::default()
    });
    fixture.owner = Some(owner.clone());
    fixture.dispatch(request("message.send", json!({"id": "task-1"}))).await;
    receive(&mut fixture.agents).await?;
    assert_eq!(owner.calls.lock().expect("owner calls").len(), 1);
    assert_eq!(fixture.audit("ok").await?["tier1_decision"], "allow");
    Ok(())
}

#[tokio::test]
async fn task_reads_authorize_the_task_without_emitting_owner() -> TestResult {
    let mut fixture = DispatchFixture::new().await?;
    let gate = Arc::new(RecordingTier1::new(Tier1AuthorizeOutcome::Allowed { zed_token: None }));
    let owner = Arc::new(RecordingOwner::default());
    fixture.tier1 = gate.clone();
    fixture.owner = Some(owner.clone());
    fixture.dispatch(request("tasks.get", json!({"id": "task-1"}))).await;
    receive(&mut fixture.agents).await?;
    let calls = gate.calls.lock().expect("authorization calls").clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].tuple.resource_type.as_str(), "task");
    assert_eq!(calls[0].tuple.resource_id.as_str(), "bot:task-1");
    assert!(owner.calls.lock().expect("owner calls").is_empty());
    fixture.audit("ok").await?;
    Ok(())
}

#[tokio::test]
async fn tier1_denials_and_transport_failures_stop_forwarding_and_owner_writes() -> TestResult {
    let mut fixture = DispatchFixture::new().await?;
    let owner = Arc::new(RecordingOwner::default());
    fixture.owner = Some(owner.clone());
    for outcome in [
        Tier1AuthorizeOutcome::Denied,
        Tier1AuthorizeOutcome::TransportError,
        Tier1AuthorizeOutcome::DeriveFailed,
    ] {
        let gate = Arc::new(RecordingTier1::new(outcome));
        fixture.tier1 = gate.clone();
        fixture.dispatch(request("message.send", json!({"id": "task-1"}))).await;
        fixture.denied(-32801, json!("request-7")).await?;
        let audit = fixture.audit("err").await?;
        assert_eq!(audit["code"], -32801);
        assert_eq!(audit["tier1_decision"], "deny");
        assert_eq!(audit["caller_id"], "alice");
        assert_eq!(audit["rules_fired"], json!(["gateway.tier1.spicedb_denied"]));
        assert_eq!(gate.calls.lock().expect("authorization calls").len(), 1);
        assert!(owner.calls.lock().expect("owner calls").is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn tier1_rejects_missing_task_coordinates_before_asking_authorizer() -> TestResult {
    let mut fixture = DispatchFixture::new().await?;
    let gate = Arc::new(RecordingTier1::new(Tier1AuthorizeOutcome::Allowed { zed_token: None }));
    fixture.tier1 = gate.clone();
    fixture.dispatch(request("tasks.get", json!({}))).await;
    fixture.denied(-32801, json!("request-7")).await?;
    assert!(gate.calls.lock().expect("authorization calls").is_empty());
    assert_eq!(
        fixture.audit("err").await?["message"],
        "tier-1 resource tuple derivation failed"
    );
    Ok(())
}

#[tokio::test]
async fn tier1_rejects_caller_without_session_identity() -> TestResult {
    let mut fixture = DispatchFixture::new().await?;
    let gate = Arc::new(RecordingTier1::new(Tier1AuthorizeOutcome::Allowed { zed_token: None }));
    fixture.tier1 = gate.clone();
    let mut message = request("message.send", json!({}));
    message
        .headers
        .as_mut()
        .expect("headers")
        .insert(GATEWAY_CALLER_ID_HEADER, "user/");
    fixture.dispatch(message).await;
    fixture.denied(-32801, json!("request-7")).await?;
    assert!(gate.calls.lock().expect("authorization calls").is_empty());
    assert_eq!(
        fixture.audit("err").await?["message"],
        "tier-1 principal lacks session identity"
    );
    Ok(())
}

#[tokio::test]
async fn declarative_policy_matches_ingress_identity_and_preserves_spicedb_snapshot() -> TestResult {
    let mut fixture = DispatchFixture::new().await?;
    fixture.tier1 = Arc::new(RecordingTier1::new(Tier1AuthorizeOutcome::Allowed {
        zed_token: Some("zed-2".into()),
    }));
    fixture.declarative = Arc::new(RealTier1DeclarativeGate::new(Tier1DeclarativeBundle::new(vec![
        Tier1DeclarativeRule {
            id: Tier1DeclarativeRuleId::new("deny-alice-send")?,
            priority: 1,
            effect: Tier1DeclarativeEffect::Deny,
            matches: vec![
                Tier1DeclarativeMatch::new(Tier1ResourceKind::CallerSubject, "user/alice", false)?,
                Tier1DeclarativeMatch::new(Tier1ResourceKind::AgentMethod, "message/send", false)?,
                Tier1DeclarativeMatch::new(
                    Tier1ResourceKind::NatsSubjectPattern,
                    "a2a.v1.gateway.bot.message.send",
                    false,
                )?,
            ],
        },
    ])));
    fixture.dispatch(request("message.send", json!({}))).await;
    fixture.denied(-32803, json!("request-7")).await?;
    let audit = fixture.audit("err").await?;
    assert_eq!(
        audit["rules_fired"],
        json!(["gateway.tier1.declarative.denied.deny-alice-send"])
    );
    assert_eq!(audit["zed_token_snapshot"], "zed-2");
    assert_eq!(audit["tier1_decision"], "deny");
    fixture.dispatch(request("tasks.get", json!({"id": "task-1"}))).await;
    receive(&mut fixture.agents).await?;
    fixture.audit("ok").await?;
    Ok(())
}

#[tokio::test]
async fn tier2_denial_stops_dispatch_and_names_the_failed_rule() -> TestResult {
    let mut fixture = DispatchFixture::new().await?;
    let directory = tempfile::tempdir()?;
    fixture.policy.substrate = Some(Arc::new(WasmtimeSubstrate::try_new_with_tier2(
        WasmBundlePath::new(directory.path()),
        Tier2State::Active(Box::new(DenyAllTier2Evaluator)),
        None,
    )?));
    fixture.dispatch(request("message.send", json!({}))).await;
    fixture.denied(-32801, json!("request-7")).await?;
    let audit = fixture.audit("err").await?;
    assert_eq!(audit["caller_id"], "alice");
    assert_eq!(audit["rules_fired"], json!(["gateway.tier2.evaluation_error"]));
    Ok(())
}

#[tokio::test]
async fn audit_distinguishes_active_tier2_allow_from_inactive_substrate() -> TestResult {
    let mut fixture = DispatchFixture::new().await?;
    let directory = tempfile::tempdir()?;
    for (tier2, rule) in [
        (
            Tier2State::Active(Box::new(NoopTier2Evaluator)),
            "gateway.tier2.evaluated_allow",
        ),
        (Tier2State::Inactive, "gateway.tier2.no_op_evaluated_true"),
    ] {
        fixture.policy.substrate = Some(Arc::new(WasmtimeSubstrate::try_new_with_tier2(
            WasmBundlePath::new(directory.path()),
            tier2,
            None,
        )?));
        fixture.dispatch(request("message.send", json!({}))).await;
        receive(&mut fixture.agents).await?;
        assert_eq!(fixture.audit("ok").await?["rules_fired"][1], rule);
    }
    Ok(())
}

struct RedactingGate {
    rewrite: RedactionRewrite,
}

impl Tier3RedactionGate for RedactingGate {
    fn redact(&self, context: &mut Tier3EvaluationContext) -> Tier3RedactionDecision {
        context.payload_mut()["params"]["secret"] = json!("[redacted]");
        Tier3RedactionDecision::Allow {
            rewrites: vec![self.rewrite.clone()],
        }
    }
}

struct RejectingGate {
    decision: Tier3RedactionDecision,
}

impl Tier3RedactionGate for RejectingGate {
    fn redact(&self, _context: &mut Tier3EvaluationContext) -> Tier3RedactionDecision {
        self.decision.clone()
    }
}

#[tokio::test]
async fn tier3_rewrites_forwarded_payload_and_records_redaction_with_route() -> TestResult {
    let diagnostics = Diagnostics::both_outputs();
    let mut fixture = DispatchFixture::new().await?;
    fixture.env.set(ENV_TIER3_REDACTION_ENABLED, "true");
    fixture.policy.tier3_gate = Arc::new(RedactingGate {
        rewrite: RedactionRewrite::new(SkillId::new("pii")?, "$.params.secret", RewriteKind::Masked)?,
    });
    fixture
        .dispatch(request("message.send", json!({"secret": "confidential"})))
        .await;
    let forwarded = receive(&mut fixture.agents).await?;
    assert_eq!(
        serde_json::from_slice::<Value>(&forwarded.payload)?["params"]["secret"],
        "[redacted]"
    );
    let audit = fixture.audit("ok").await?;
    assert_eq!(audit["tier3_decision"], "allow");
    assert_eq!(audit["rules_fired"][2], "gateway.tier3.redacted");
    assert_eq!(audit["rewrites"][1], "pii:Masked@$.params.secret");
    assert!(!serde_json::to_string(&audit)?.contains("confidential"));
    diagnostics.assert_event(
        "gateway tier-3 redaction applied before forward",
        &[("caller_id", "alice"), ("method", "message/send"), ("count", "1")],
    );
    Ok(())
}

#[tokio::test]
async fn tier3_refusal_and_engine_failure_fail_closed_with_distinct_protocol_errors() -> TestResult {
    let diagnostics = Diagnostics::both_outputs();
    let mut fixture = DispatchFixture::new().await?;
    for (decision, code, tier3, rule) in [
        (
            Tier3RedactionDecision::Refuse {
                reason: Tier3RefusalReason::UnauthorizedDataCategory,
                rule: SkillId::new("pii")?,
            },
            -32802,
            "refuse",
            "gateway.tier3.refused.pii",
        ),
        (
            Tier3RedactionDecision::Error {
                kind: Tier3EngineError::WasmTrap,
                rule: SkillId::new("pii")?,
            },
            -32801,
            "error",
            "gateway.tier3.engine_error",
        ),
    ] {
        fixture.policy.tier3_gate = Arc::new(RejectingGate { decision });
        fixture.dispatch(request("message.send", json!({}))).await;
        let body = fixture.denied(code, json!("request-7")).await?;
        if code == -32802 {
            assert_eq!(body["error"]["data"]["rule"], "pii");
        }
        let audit = fixture.audit("err").await?;
        assert_eq!(audit["code"], code);
        assert_eq!(audit["tier3_decision"], tier3);
        assert_eq!(audit["rules_fired"], json!([rule]));
    }
    diagnostics.assert_event(
        "gateway tier-3 skill refused part redaction",
        &[("caller_id", "alice"), ("method", "message/send"), ("skill_id", "pii")],
    );
    diagnostics.assert_event(
        "gateway tier-3 redaction engine failed closed",
        &[("caller_id", "alice"), ("method", "message/send"), ("skill_id", "pii")],
    );
    Ok(())
}

#[tokio::test]
async fn oversized_forward_is_audited_as_publish_failure_for_unary_and_task_methods() -> TestResult {
    let mut fixture = DispatchFixture::new().await?;
    let oversized = "x".repeat(fixture.client.server_info().max_payload + 1);
    for method in ["message.send", "tasks.get"] {
        fixture
            .dispatch(request(method, json!({"id": "task-1", "data": oversized})))
            .await;
        let audit = fixture.audit("err").await?;
        assert_eq!(audit["code"], -32803);
        assert_eq!(audit["req_id"], "request-7");
        assert_empty(&fixture.client, &mut fixture.agents, "a2a.v1.agents.barrier").await?;
        assert_empty(&fixture.client, &mut fixture.replies, "_INBOX.dispatch").await?;
    }
    Ok(())
}
