use buffa::{EnumValue, Message, MessageField};
use buffa_types::google::protobuf::Timestamp;
use trogonai_proto::platform::audit::v1::audit_actor::Unattributed;
use trogonai_proto::platform::audit::v1::audit_record::Detail;
use trogonai_proto::platform::audit::v1::{AuditActor, AuditClass, EnforcementMode, GatewaySurface};
use trogonai_proto::platform::workload_identity::v1alpha1::{RegistrationBinding, WorkloadPeerEvidence, WorkloadRole};

use crate::fixtures::{body, decision_facts, identity, name, resealed, scope, scope_for, stamper, token};
use crate::{
    Actor, AuditWriter, ChainAnchor, ChainVerificationError, DecisionOutcome, Enforcement, MemorySink, Obligation,
    ReadOnly, Record, RecordScope, Surface, WorkloadPeerEvidenceFacts, WorkloadPeerFactsError, WriterConfig,
    verify_chain_range,
};

fn peer() -> WorkloadPeerEvidence {
    WorkloadPeerEvidence {
        peer_binding: MessageField::some(RegistrationBinding {
            spiffe_id: "spiffe://example.test/runtime/one".into(),
            tenant: "tenant_1".into(),
            project: "project_1".into(),
            role: EnumValue::from(WorkloadRole::Agent),
            application_id: Some("agent-7".into()),
        }),
        peer_leaf_sha256: vec![1; 32],
        peer_path_expires_at: MessageField::some(Timestamp {
            seconds: 1_800_000_060,
            nanos: 0,
            ..Default::default()
        }),
        recovery_fence: 1,
        registration_revision: 2,
        trust_revision: 3,
        verifier_source_revision: 4,
        verifier_activation_revision: 5,
        verifier_bundle_sha256: vec![6; 32],
    }
}

fn evidence(record: &Record) -> &WorkloadPeerEvidence {
    let Some(Detail::GatewayDecision(decision)) = record.message().detail.as_ref() else {
        panic!("gateway detail");
    };
    decision.workload_peer.as_option().expect("authenticated peer evidence")
}

#[test]
fn static_legacy_decisions_cannot_claim_authenticated_workload_evidence() {
    let record = Record::decision(
        scope(),
        decision_facts(DecisionOutcome::Deny, ReadOnly::Undeclared),
        stamper().stamp(),
    );
    let Some(Detail::GatewayDecision(decision)) = record.message().detail.as_ref() else {
        panic!("gateway detail");
    };
    assert!(decision.workload_peer.is_unset());
}

#[test]
fn authenticated_records_distinguish_runtimes_that_share_one_application_identity() {
    let first = peer();
    let mut second = peer();
    second.peer_binding.as_option_mut().unwrap().spiffe_id = "spiffe://example.test/runtime/two".into();
    second.peer_leaf_sha256 = vec![2; 32];
    let facts = || decision_facts(DecisionOutcome::Allow(Enforcement::Enforce), ReadOnly::Undeclared);
    let inspected = WorkloadPeerEvidenceFacts::try_from(first.clone()).unwrap();
    assert_eq!(inspected.message(), &first);
    let first_record = Record::authenticated_workload_decision(scope(), facts(), inspected, stamper().stamp()).unwrap();
    let second_record = Record::authenticated_workload_decision(
        scope(),
        facts(),
        WorkloadPeerEvidenceFacts::try_from(second.clone()).unwrap(),
        stamper().stamp(),
    )
    .unwrap();
    assert_eq!(first_record.message().actor, second_record.message().actor);
    assert_eq!(evidence(&first_record), &first);
    assert_eq!(evidence(&second_record), &second);
    assert_ne!(evidence(&first_record), evidence(&second_record));
}

#[test]
fn an_expiry_refusal_retains_the_previously_authenticated_peer() {
    let mut message = peer();
    message.peer_path_expires_at.as_option_mut().unwrap().seconds = 1_799_999_999;
    let record = Record::authenticated_workload_decision(
        scope(),
        decision_facts(DecisionOutcome::Deny, ReadOnly::Undeclared),
        WorkloadPeerEvidenceFacts::try_from(message.clone()).unwrap(),
        stamper().stamp(),
    )
    .unwrap();
    assert_eq!(record.obligation(), Obligation::Stands);
    assert_eq!(evidence(&record), &message);
}

#[test]
fn workload_evidence_refuses_malformed_authenticated_metadata() {
    let mut message = peer();
    message.peer_leaf_sha256.pop();
    assert_eq!(
        WorkloadPeerEvidenceFacts::try_from(message),
        Err(WorkloadPeerFactsError::Fingerprint)
    );
}

#[test]
fn checked_workload_decisions_refuse_scope_actor_and_profile_confusion() {
    let peer = WorkloadPeerEvidenceFacts::try_from(peer()).unwrap();
    let facts = || decision_facts(DecisionOutcome::Allow(Enforcement::Enforce), ReadOnly::Undeclared);
    for bad_scope in [
        RecordScope::new(name("tenant_2"), token("project_1"), scope().actor().clone()),
        RecordScope::new(name("tenant_1"), token("project_2"), scope().actor().clone()),
        scope_for(Actor::Unattributed),
        scope_for(Actor::Agent {
            agent_id: name("agent-8"),
        }),
    ] {
        assert!(Record::authenticated_workload_decision(bad_scope, facts(), peer.clone(), stamper().stamp()).is_err());
    }
    let mut another_surface = facts();
    another_surface.surface = Surface::A2aIngress;
    for bad_facts in [
        another_surface,
        decision_facts(DecisionOutcome::Allow(Enforcement::DryRun), ReadOnly::Undeclared),
        decision_facts(DecisionOutcome::WouldDeny, ReadOnly::Undeclared),
        decision_facts(DecisionOutcome::Allow(Enforcement::Enforce), ReadOnly::Declared),
        decision_facts(DecisionOutcome::Deny, ReadOnly::Declared),
    ] {
        assert!(Record::authenticated_workload_decision(scope(), bad_facts, peer.clone(), stamper().stamp()).is_err());
    }
}

#[test]
fn peer_evidence_enforces_every_generation_and_validity_boundary() {
    let original = peer();
    let invalid = [
        (
            WorkloadPeerEvidence {
                peer_binding: MessageField::none(),
                ..original.clone()
            },
            WorkloadPeerFactsError::Binding,
        ),
        (
            WorkloadPeerEvidence {
                peer_leaf_sha256: vec![],
                ..original.clone()
            },
            WorkloadPeerFactsError::Fingerprint,
        ),
        (
            WorkloadPeerEvidence {
                verifier_bundle_sha256: vec![1; 33],
                ..original.clone()
            },
            WorkloadPeerFactsError::Fingerprint,
        ),
        (
            WorkloadPeerEvidence {
                recovery_fence: 0,
                ..original.clone()
            },
            WorkloadPeerFactsError::Generation,
        ),
        (
            WorkloadPeerEvidence {
                registration_revision: 0,
                ..original.clone()
            },
            WorkloadPeerFactsError::Generation,
        ),
        (
            WorkloadPeerEvidence {
                trust_revision: 0,
                ..original.clone()
            },
            WorkloadPeerFactsError::Generation,
        ),
        (
            WorkloadPeerEvidence {
                verifier_source_revision: 0,
                ..original.clone()
            },
            WorkloadPeerFactsError::Generation,
        ),
        (
            WorkloadPeerEvidence {
                verifier_activation_revision: 0,
                ..original.clone()
            },
            WorkloadPeerFactsError::Generation,
        ),
        (
            WorkloadPeerEvidence {
                peer_path_expires_at: MessageField::none(),
                ..original.clone()
            },
            WorkloadPeerFactsError::Validity,
        ),
        (
            WorkloadPeerEvidence {
                peer_path_expires_at: MessageField::some(Timestamp {
                    seconds: 0,
                    ..Default::default()
                }),
                ..original.clone()
            },
            WorkloadPeerFactsError::Validity,
        ),
    ];
    for (message, expected) in invalid {
        assert_eq!(WorkloadPeerEvidenceFacts::try_from(message), Err(expected));
    }
    let mut bad_role = original.clone();
    bad_role.peer_binding.as_option_mut().unwrap().role = EnumValue::from(WorkloadRole::Supervisor);
    bad_role.peer_binding.as_option_mut().unwrap().application_id = None;
    assert_eq!(
        WorkloadPeerEvidenceFacts::try_from(bad_role),
        Err(WorkloadPeerFactsError::Binding)
    );
}

#[tokio::test]
async fn stored_workload_decisions_verify_and_refuse_rehashed_identity_confusion() {
    let sink = MemorySink::new();
    let writer = AuditWriter::new(sink.clone(), identity(), WriterConfig::default(), &stamper());
    let record = Record::authenticated_workload_decision(
        scope(),
        decision_facts(DecisionOutcome::Deny, ReadOnly::Undeclared),
        WorkloadPeerEvidenceFacts::try_from(peer()).unwrap(),
        stamper().stamp(),
    )
    .unwrap();
    writer.write(record).await.unwrap();
    let records = sink.stored_records();
    let anchor = ChainAnchor::Genesis {
        writer: identity().writer().clone(),
        chain: writer.chain(),
    };
    assert!(verify_chain_range(anchor.clone(), records.clone()).is_ok());
    let original = body(&records[1]);
    let mut tampered = Vec::new();
    let mut message = original.clone();
    message.tenant.as_option_mut().unwrap().value = "tenant_2".into();
    tampered.push(message);
    let mut message = original.clone();
    message.class = EnumValue::from(AuditClass::BestEffort);
    tampered.push(message);
    let mut message = original.clone();
    message.actor = MessageField::some(AuditActor {
        kind: Unattributed::default().into(),
    });
    tampered.push(message);
    let mut message = original.clone();
    message.attribution.as_option_mut().unwrap().enforcement_mode = EnumValue::from(EnforcementMode::DryRun);
    tampered.push(message);
    let mut message = original.clone();
    let Some(Detail::GatewayDecision(decision)) = message.detail.as_mut() else {
        panic!("gateway detail");
    };
    decision.surface = EnumValue::from(GatewaySurface::A2aIngress);
    tampered.push(message);
    let mut message = original.clone();
    let Some(Detail::GatewayDecision(decision)) = message.detail.as_mut() else {
        panic!("gateway detail");
    };
    decision.workload_peer.as_option_mut().unwrap().peer_leaf_sha256.pop();
    tampered.push(message);
    let mut message = original.clone();
    let Some(Detail::GatewayDecision(decision)) = message.detail.as_mut() else {
        panic!("gateway detail");
    };
    decision
        .workload_peer
        .as_option_mut()
        .unwrap()
        .peer_binding
        .as_option_mut()
        .unwrap()
        .application_id = Some("agent-8".into());
    tampered.push(message);
    for message in tampered {
        let rewritten = resealed(&records[1], &message);
        assert_eq!(
            verify_chain_range(anchor.clone(), vec![records[0].clone(), rewritten]),
            Err(ChainVerificationError::SubjectMismatch { stream_sequence: 2 })
        );
    }
}

#[test]
fn workload_peer_facts_reject_unknown_timestamp_bytes() {
    let mut message = peer();
    let mut bytes = message.peer_path_expires_at.encode_to_vec();
    bytes.extend_from_slice(&[0x7a, 1, b'A']);
    let timestamp = Timestamp::decode_from_slice(&bytes).unwrap();
    assert!(!timestamp.__buffa_unknown_fields.is_empty());
    message.peer_path_expires_at = MessageField::some(timestamp);
    assert_eq!(
        WorkloadPeerEvidenceFacts::try_from(message),
        Err(WorkloadPeerFactsError::Validity)
    );
}
