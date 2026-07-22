use super::*;

fn agent() -> A2aAgentId {
    A2aAgentId::new("test-agent").unwrap()
}

#[test]
fn ok_outcome_serializes_correctly() {
    let env = AuditEnvelope::new(
        &agent(),
        "message/send",
        Some("r1".into()),
        1000,
        5,
        AuditOutcome::Ok,
        None,
        AuditEnvelopeFields::default(),
    );
    let v = serde_json::to_value(&env).unwrap();
    assert_eq!(v["outcome"], "ok");
    assert_eq!(v["agent_id"], "test-agent");
    assert_eq!(v["method"], "message/send");
    assert_eq!(v["req_id"], "r1");
    assert_eq!(v["latency_ms"], 5);
    assert!(v["params_fingerprint"].is_null());
    assert!(v.get("trace_id").is_none());
    assert!(v.get("rules_fired").is_none());
    assert!(v.get("rewrites").is_none());
    assert!(v.get("stream_consumer").is_none());
}

#[test]
fn err_outcome_serializes_correctly() {
    let env = AuditEnvelope::new(
        &agent(),
        "tasks/get",
        None,
        2000,
        10,
        AuditOutcome::Err {
            code: -32001,
            message: "not found".into(),
        },
        Some(b"some params"),
        AuditEnvelopeFields::default(),
    );
    let v = serde_json::to_value(&env).unwrap();
    assert_eq!(v["outcome"], "err");
    assert_eq!(v["code"], -32001);
    assert_eq!(v["message"], "not found");
    assert!(v["params_fingerprint"].is_string());
    assert!(!v["params_fingerprint"].as_str().unwrap().is_empty());
}

#[test]
fn params_fingerprint_is_none_for_empty_params() {
    let env = AuditEnvelope::new(
        &agent(),
        "tasks/get",
        None,
        0,
        0,
        AuditOutcome::Ok,
        Some(b""),
        AuditEnvelopeFields::default(),
    );
    assert!(env.params_fingerprint.is_none());
}

#[test]
fn params_fingerprint_is_deterministic() {
    let a = AuditEnvelope::new(
        &agent(),
        "m",
        None,
        0,
        0,
        AuditOutcome::Ok,
        Some(b"hello"),
        AuditEnvelopeFields::default(),
    );
    let b = AuditEnvelope::new(
        &agent(),
        "m",
        None,
        0,
        0,
        AuditOutcome::Ok,
        Some(b"hello"),
        AuditEnvelopeFields::default(),
    );
    assert_eq!(a.params_fingerprint, b.params_fingerprint);
}

#[test]
fn params_fingerprint_differs_for_different_params() {
    let a = AuditEnvelope::new(
        &agent(),
        "m",
        None,
        0,
        0,
        AuditOutcome::Ok,
        Some(b"hello"),
        AuditEnvelopeFields::default(),
    );
    let b = AuditEnvelope::new(
        &agent(),
        "m",
        None,
        0,
        0,
        AuditOutcome::Ok,
        Some(b"world"),
        AuditEnvelopeFields::default(),
    );
    assert_ne!(a.params_fingerprint, b.params_fingerprint);
}

#[test]
fn audit_subject_rewrite_formats_ingress_to_agent() {
    let rewrite = AuditSubjectRewrite::new("a2a.gateway.bot.message.send", "a2a.agents.bot.message.send");
    assert_eq!(
        rewrite.as_str(),
        "ingress:a2a.gateway.bot.message.send -> agent:a2a.agents.bot.message.send"
    );
    let json = rewrite.into_audit_json();
    assert_eq!(
        json,
        serde_json::json!(["ingress:a2a.gateway.bot.message.send -> agent:a2a.agents.bot.message.send"])
    );
}

#[test]
fn gateway_stream_consumer_name_for_sse_methods_only() {
    let agent = agent();
    assert_eq!(
        GatewayStreamConsumerName::for_sse_method(&agent, "message.stream")
            .unwrap()
            .as_str(),
        "gateway.test-agent.message.stream"
    );
    assert_eq!(
        GatewayStreamConsumerName::for_sse_method(&agent, "tasks.resubscribe")
            .unwrap()
            .as_str(),
        "gateway.test-agent.tasks.resubscribe"
    );
    assert!(GatewayStreamConsumerName::for_sse_method(&agent, "message.send").is_none());
    assert!(GatewayStreamConsumerName::for_sse_method(&agent, "tasks.get").is_none());
}

#[test]
fn gateway_forward_audit_extras_populates_rewrite_and_sse_consumer() {
    let agent = agent();
    let (rewrites, stream_consumer) = gateway_forward_audit_extras(
        "a2a.gateway.bot.message.stream",
        "a2a.agents.bot.message.stream",
        &agent,
        "message.stream",
    );
    assert_eq!(
        rewrites,
        Some(serde_json::json!([
            "ingress:a2a.gateway.bot.message.stream -> agent:a2a.agents.bot.message.stream"
        ]))
    );
    assert_eq!(stream_consumer.as_deref(), Some("gateway.test-agent.message.stream"));
}

#[test]
fn gateway_forward_audit_extras_omits_stream_consumer_for_unary() {
    let agent = agent();
    let (rewrites, stream_consumer) = gateway_forward_audit_extras(
        "a2a.gateway.bot.message.send",
        "a2a.agents.bot.message.send",
        &agent,
        "message.send",
    );
    assert_eq!(
        rewrites,
        Some(serde_json::json!([
            "ingress:a2a.gateway.bot.message.send -> agent:a2a.agents.bot.message.send"
        ]))
    );
    assert!(stream_consumer.is_none());
}

#[test]
fn optional_fields_omit_when_none_and_include_trace_id_when_set() {
    let default_env = AuditEnvelope::new(
        &agent(),
        "message/send",
        None,
        0,
        0,
        AuditOutcome::Ok,
        None,
        AuditEnvelopeFields::default(),
    );
    let default_json = serde_json::to_value(&default_env).unwrap();
    assert!(default_json.get("trace_id").is_none());

    let extras = AuditEnvelopeFields {
        trace_id: Some("trace-abc".into()),
        ..Default::default()
    };
    let traced_env = AuditEnvelope::new(&agent(), "message/send", None, 0, 0, AuditOutcome::Ok, None, extras);
    let traced_json = serde_json::to_value(&traced_env).unwrap();
    assert_eq!(traced_json["trace_id"], "trace-abc");
    assert!(traced_json.get("rules_fired").is_none());
    assert!(traced_json.get("rewrites").is_none());
    assert!(traced_json.get("stream_consumer").is_none());
}
