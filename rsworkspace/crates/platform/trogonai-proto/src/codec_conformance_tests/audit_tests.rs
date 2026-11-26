use buffa::Message;
use serde_json::json;

use super::{assert_json_codec, assert_wire_codec};
use crate::platform::audit::v1::{
    AuditCheckpoint, AuditClass, AuditRecord, ChainResetReason, Change, ChangeKind, CheckpointStatement, CredentialUse,
    CredentialUseKind, Integrity, WriterLifecycle, WriterLifecycleKind,
};

#[test]
fn gateway_decision_record_round_trips_through_json_and_wire() {
    let record = assert_json_codec::<AuditRecord>(json!({
        "recordId": "01939a4e-7b1c-7d2e-8f3a-5b6c7d8e9f01",
        "class": "AUDIT_CLASS_MUST",
        "occurredAt": "2026-11-03T10:00:00Z",
        "recordedAt": "2026-11-03T10:00:01Z",
        "tenant": {"value": "node_tenant"},
        "project": {"value": "project_1"},
        "actor": {"agent": {"agentId": {"value": "agent_1"}}},
        "actChain": [{"agentId": {"value": "agent_0"}}],
        "traceId": "4bf92f3577b34da6a3ce929d0e0e4736",
        "requestId": "req_1",
        "attribution": {
            "decision": "AUDIT_DECISION_DENY",
            "enforcementMode": "ENFORCEMENT_MODE_ENFORCE",
            "reasonCode": "EGRESS_NOT_PERMITTED",
            "wouldDeny": false,
            "ruleIds": ["rule_1"],
            "policyRuleRefs": [{
                "kind": "POLICY_RULE_KIND_HOLD",
                "subject": {"agentId": {"value": "agent_1"}},
                "streamRevision": "7"
            }],
            "policySource": "policy_projection",
            "projectionRevision": "42"
        },
        "integrity": {
            "writerId": "egress-proxy-1",
            "sequence": "9",
            "previousHash": "AQID",
            "recordHash": "BAUG"
        },
        "gatewayDecision": {
            "surface": "GATEWAY_SURFACE_EGRESS",
            "target": "api.example.com:443",
            "requestBytes": "120",
            "responseBytes": "0",
            "latency": "0.012s",
            "tlsMode": "TLS_MODE_PASSTHROUGH",
            "layerVerdicts": [{
                "layer": "egress_permissions",
                "decision": "AUDIT_DECISION_DENY",
                "enforcementMode": "ENFORCEMENT_MODE_ENFORCE",
                "wouldDeny": false
            }],
            "session": {"value": "session_1"}
        }
    }));

    assert_eq!(record.class.as_known(), Some(AuditClass::AUDIT_CLASS_MUST));
    assert_eq!(record.act_chain.len(), 1);
}

#[test]
fn credential_use_record_round_trips_through_json_and_wire() {
    assert_json_codec::<AuditRecord>(json!({
        "recordId": "01939a4e-7b1c-7d2e-8f3a-5b6c7d8e9f02",
        "class": "AUDIT_CLASS_MUST",
        "occurredAt": "2026-11-03T10:00:00Z",
        "recordedAt": "2026-11-03T10:00:01Z",
        "tenant": {"value": "node_tenant"},
        "project": {"value": "project_1"},
        "actor": {"unattributed": {}},
        "attribution": {
            "decision": "AUDIT_DECISION_ALLOW",
            "enforcementMode": "ENFORCEMENT_MODE_ENFORCE",
            "wouldDeny": false
        },
        "credentialUse": {
            "kind": "CREDENTIAL_USE_KIND_INJECT",
            "credential": "projects/p1/credentials/c1",
            "bindingId": "binding_1",
            "destination": "api.example.com",
            "decisionRecordId": "01939a4e-7b1c-7d2e-8f3a-5b6c7d8e9f01"
        }
    }));
}

#[test]
fn change_record_points_at_an_event() {
    let record = assert_json_codec::<AuditRecord>(json!({
        "recordId": "01939a4e-7b1c-7d2e-8f3a-5b6c7d8e9f03",
        "class": "AUDIT_CLASS_MUST",
        "occurredAt": "2026-11-03T10:00:00Z",
        "recordedAt": "2026-11-03T10:00:01Z",
        "tenant": {"value": "node_tenant"},
        "project": {"value": "project_1"},
        "actor": {"breakGlass": {"operator": "op", "host": "host_1"}},
        "change": {
            "kind": "CHANGE_KIND_CREDENTIAL",
            "resourceName": "projects/p1/credentials/c1",
            "eventType": "credential.revoked",
            "sourceStream": "CREDENTIALS",
            "streamSequence": "314",
            "eventId": "evt_1"
        }
    }));

    assert!(record.attribution.is_unset());
}

#[test]
fn change_wire_layout_is_stable() {
    let wire = [
        0x08, 0x01, // kind = POLICY
        0x12, 0x02, b'r', b'1', // resource_name
        0x1a, 0x02, b'e', b't', // event_type
        0x22, 0x01, b's', // source_stream
        0x28, 0x05, // stream_sequence = 5
        0x32, 0x01, b'i', // event_id
    ];
    let change = Change {
        kind: ChangeKind::CHANGE_KIND_POLICY.into(),
        resource_name: "r1".to_string(),
        event_type: "et".to_string(),
        source_stream: "s".to_string(),
        stream_sequence: 5,
        event_id: "i".to_string(),
    };

    assert_eq!(change.try_encode_to_vec().expect("encode"), wire);
    assert_wire_codec(&wire, &change);
}

#[test]
fn credential_use_names_a_credential_and_never_holds_one() {
    let json = serde_json::to_value(CredentialUse {
        kind: CredentialUseKind::CREDENTIAL_USE_KIND_RELEASE.into(),
        credential: "projects/p1/credentials/c1".to_string(),
        binding_id: "b".to_string(),
        destination: Some("api.example.com".to_string()),
        decision_record_id: Some("01939a4e-7b1c-7d2e-8f3a-5b6c7d8e9f01".to_string()),
        location: Some("header:authorization".to_string()),
    })
    .expect("json");
    let mut keys: Vec<_> = json.as_object().expect("object").keys().cloned().collect();
    keys.sort();

    assert_eq!(
        keys,
        [
            "bindingId",
            "credential",
            "decisionRecordId",
            "destination",
            "kind",
            "location"
        ]
    );
}

#[test]
fn a_secrets_release_names_its_service_caller_and_where_the_credential_goes() {
    let record = assert_json_codec::<AuditRecord>(json!({
        "recordId": "01939a4e-7b1c-7d2e-8f3a-5b6c7d8e9f05",
        "class": "AUDIT_CLASS_MUST",
        "occurredAt": "2026-11-03T10:00:00Z",
        "recordedAt": "2026-11-03T10:00:01Z",
        "tenant": {"value": "tenant_1"},
        "project": {"value": "project_1"},
        "actor": {"service": {"serviceId": "billing-runner"}},
        "requestId": "use-1",
        "attribution": {
            "decision": "AUDIT_DECISION_ALLOW",
            "enforcementMode": "ENFORCEMENT_MODE_ENFORCE",
            "wouldDeny": false
        },
        "credentialUse": {
            "kind": "CREDENTIAL_USE_KIND_RELEASE",
            "credential": "projects/p1/credentials/c1",
            "bindingId": "projects/p1/credentials/c1",
            "destination": "api.example.com:443",
            "location": "header:authorization"
        }
    }));

    assert_eq!(record.request_id.as_deref(), Some("use-1"));
}

#[test]
fn a_secrets_release_denial_is_on_its_own_surface() {
    assert_json_codec::<AuditRecord>(json!({
        "recordId": "01939a4e-7b1c-7d2e-8f3a-5b6c7d8e9f06",
        "class": "AUDIT_CLASS_MUST",
        "occurredAt": "2026-11-03T10:00:00Z",
        "recordedAt": "2026-11-03T10:00:01Z",
        "tenant": {"value": "tenant_1"},
        "project": {"value": "project_1"},
        "actor": {"service": {"serviceId": "billing-runner"}},
        "attribution": {
            "decision": "AUDIT_DECISION_DENY",
            "enforcementMode": "ENFORCEMENT_MODE_ENFORCE",
            "wouldDeny": false,
            "reasonCode": "owner_not_held"
        },
        "gatewayDecision": {
            "surface": "GATEWAY_SURFACE_SECRETS_RELEASE",
            "target": "projects/p1/credentials/c1"
        }
    }));
}

#[test]
fn integrity_round_trips_hashes_as_bytes() {
    let integrity = assert_json_codec::<Integrity>(json!({
        "writerId": "w1",
        "sequence": "1",
        "recordHash": "AQID"
    }));

    assert_eq!(integrity.record_hash, Some(vec![1, 2, 3]));
    assert_eq!(integrity.previous_hash, None);
}

#[test]
fn integrity_round_trips_the_chain_id() {
    let integrity = assert_json_codec::<Integrity>(json!({
        "writerId": "w1",
        "sequence": "2",
        "previousHash": "AQID",
        "recordHash": "BAUG",
        "chainId": "01939a4e-7b1c-7d2e-8f3a-5b6c7d8e9f04"
    }));

    assert_eq!(
        integrity.chain_id.as_deref(),
        Some("01939a4e-7b1c-7d2e-8f3a-5b6c7d8e9f04")
    );
}

#[test]
fn chain_reset_record_carries_no_attribution() {
    let record = assert_json_codec::<AuditRecord>(json!({
        "recordId": "01939a4e-7b1c-7d2e-8f3a-5b6c7d8e9f04",
        "class": "AUDIT_CLASS_MUST",
        "occurredAt": "2026-11-03T10:00:00Z",
        "recordedAt": "2026-11-03T10:00:01Z",
        "tenant": {"value": "node_tenant"},
        "project": {"value": "project_1"},
        "actor": {"unattributed": {}},
        "integrity": {
            "writerId": "egress-proxy-1",
            "sequence": "1",
            "chainId": "01939a4e-7b1c-7d2e-8f3a-5b6c7d8e9f04"
        },
        "writerLifecycle": {
            "kind": "WRITER_LIFECYCLE_KIND_CHAIN_RESET",
            "resetReason": "CHAIN_RESET_REASON_PROCESS_START_WITHOUT_RESUME"
        }
    }));

    assert!(record.attribution.is_unset());
}

#[test]
fn writer_lifecycle_wire_layout_is_stable() {
    let wire = [
        0x08, 0x02, // kind = CHAIN_RESET
        0x10, 0x01, // reset_reason = PROCESS_START_WITHOUT_RESUME
    ];
    let lifecycle = WriterLifecycle {
        kind: WriterLifecycleKind::WRITER_LIFECYCLE_KIND_CHAIN_RESET.into(),
        reset_reason: Some(ChainResetReason::CHAIN_RESET_REASON_PROCESS_START_WITHOUT_RESUME.into()),
    };

    assert_eq!(lifecycle.try_encode_to_vec().expect("encode"), wire);
    assert_wire_codec(&wire, &lifecycle);
}

#[test]
fn checkpoint_statement_round_trips_through_json_and_wire() {
    let statement = assert_json_codec::<CheckpointStatement>(json!({
        "writerId": "egress-proxy-1",
        "chainId": "01939a4e-7b1c-7d2e-8f3a-5b6c7d8e9f04",
        "headSequence": "1000",
        "headHash": "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE=",
        "auditStream": "AUDIT",
        "auditStreamSequence": "31415",
        "issuedAt": "2026-11-03T10:01:00Z",
        "keyId": "did:key:z6MkhaXgBZDvotDkL5257faiztiGiC2QtKLGpbnnEGta2doK",
        "checkpointId": "01939a4e-7b1c-7d2e-8f3a-5b6c7d8e9f05",
        "component": "egress_proxy"
    }));

    assert_eq!(statement.head_hash, vec![1; 32]);
    assert_eq!(statement.audit_stream_sequence, 31415);
}

#[test]
fn audit_checkpoint_carries_the_statement_bytes_verbatim() {
    let statement = CheckpointStatement {
        writer_id: "egress-proxy-1".to_string(),
        chain_id: "01939a4e-7b1c-7d2e-8f3a-5b6c7d8e9f04".to_string(),
        head_sequence: 1,
        head_hash: vec![1; 32],
        audit_stream: "AUDIT".to_string(),
        audit_stream_sequence: 7,
        key_id: "did:key:z6Mk".to_string(),
        checkpoint_id: "01939a4e-7b1c-7d2e-8f3a-5b6c7d8e9f05".to_string(),
        component: "egress_proxy".to_string(),
        ..Default::default()
    };
    let statement_bytes = statement.try_encode_to_vec().expect("encode");
    let checkpoint = AuditCheckpoint {
        statement: statement_bytes.clone(),
        detached_jws: "eyJhbGciOiJFZERTQSJ9..c2ln".to_string(),
    };

    let decoded = AuditCheckpoint::decode_from_slice(&checkpoint.try_encode_to_vec().expect("encode")).expect("decode");
    assert_eq!(decoded.statement, statement_bytes);
    assert_eq!(
        CheckpointStatement::decode_from_slice(&decoded.statement).expect("statement"),
        statement
    );

    assert_json_codec::<AuditCheckpoint>(json!({
        "statement": "CgFh",
        "detachedJws": "eyJhbGciOiJFZERTQSJ9..c2ln"
    }));
}

#[test]
fn truncated_audit_record_fails_closed() {
    let wire = [0x0a, 0x24, b'0', b'1'];

    assert!(AuditRecord::decode_from_slice(&wire).is_err());
}
