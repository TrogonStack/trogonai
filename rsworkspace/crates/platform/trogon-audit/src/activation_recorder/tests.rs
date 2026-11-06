use buffa::{EnumValue, Message, MessageField};
use buffa_types::google::protobuf::Timestamp;
use trogonai_proto::platform::audit::v1::AuditClass;
use trogonai_proto::platform::audit::v1::audit_record::Detail;
use trogonai_proto::platform::workload_identity::v1alpha1::{CredentialActivation, RegistrationBinding, WorkloadRole};

use crate::fixtures::{body, identity, name, resealed, scope, stamper, token};
use crate::{
    ActivationFactsError, ActivationRecorder, ActivationRecorderError, Actor, AuditWriter, ChainAnchor,
    ChainVerificationError, CredentialActivationFacts, MemorySink, Obligation, Record, RecordScope, SinkError,
    WriterConfig, verify_chain_range,
};

fn activation() -> CredentialActivation {
    CredentialActivation {
        binding: MessageField::some(RegistrationBinding {
            spiffe_id: "spiffe://example.test/proxy".into(),
            tenant: "tenant_1".into(),
            project: "project_1".into(),
            role: EnumValue::from(WorkloadRole::EgressProxy),
            application_id: None,
        }),
        recovery_fence: 1,
        registration_revision: 2,
        trust_revision: 3,
        source_revision: 4,
        activation_revision: 4,
        leaf_sha256: vec![1; 32],
        public_key_sha256: vec![2; 32],
        bundle_sha256: vec![3; 32],
        not_before: MessageField::some(Timestamp {
            seconds: 1_800_000_000,
            nanos: 0,
            ..Default::default()
        }),
        expires_at: MessageField::some(Timestamp {
            seconds: 1_800_003_600,
            nanos: 0,
            ..Default::default()
        }),
    }
}

#[test]
fn activation_record_preserves_facts_and_requires_storage_before_effect() {
    let message = activation();
    let facts = CredentialActivationFacts::try_from(message.clone()).unwrap();
    let record = Record::credential_activation(scope(), facts, stamper().stamp()).unwrap();
    assert_eq!(record.message().class, EnumValue::from(AuditClass::Must));
    assert_eq!(record.obligation(), Obligation::BeforeEffect);
    assert_eq!(
        record.subject().render(),
        "platform.v1.audit.change.project_1.credential"
    );
    assert!(
        matches!(record.message().detail.as_ref(), Some(Detail::CredentialActivation(actual)) if **actual == message)
    );
}

#[test]
fn activation_facts_do_not_rely_on_inert_schema_annotations() {
    let mut malformed = activation();
    malformed.public_key_sha256.pop();
    assert_eq!(
        CredentialActivationFacts::try_from(malformed),
        Err(ActivationFactsError::Fingerprint)
    );
}

#[test]
fn activation_facts_refuse_unsupported_authority_and_validity() {
    let original = activation();
    let invalid = [
        (
            CredentialActivation {
                binding: MessageField::none(),
                ..original.clone()
            },
            ActivationFactsError::Binding,
        ),
        (
            CredentialActivation {
                source_revision: 0,
                ..original.clone()
            },
            ActivationFactsError::Generation,
        ),
        (
            CredentialActivation {
                recovery_fence: 0,
                ..original.clone()
            },
            ActivationFactsError::Generation,
        ),
        (
            CredentialActivation {
                registration_revision: 0,
                ..original.clone()
            },
            ActivationFactsError::Generation,
        ),
        (
            CredentialActivation {
                trust_revision: 0,
                ..original.clone()
            },
            ActivationFactsError::Generation,
        ),
        (
            CredentialActivation {
                activation_revision: 0,
                ..original.clone()
            },
            ActivationFactsError::Generation,
        ),
        (
            CredentialActivation {
                bundle_sha256: vec![1; 33],
                ..original.clone()
            },
            ActivationFactsError::Fingerprint,
        ),
        (
            CredentialActivation {
                leaf_sha256: vec![],
                ..original.clone()
            },
            ActivationFactsError::Fingerprint,
        ),
        (
            CredentialActivation {
                not_before: MessageField::none(),
                ..original.clone()
            },
            ActivationFactsError::Validity,
        ),
        (
            CredentialActivation {
                expires_at: MessageField::none(),
                ..original.clone()
            },
            ActivationFactsError::Validity,
        ),
        (
            CredentialActivation {
                expires_at: original.not_before.clone(),
                ..original.clone()
            },
            ActivationFactsError::Validity,
        ),
        (
            CredentialActivation {
                expires_at: MessageField::some(Timestamp {
                    seconds: 1_800_003_601,
                    nanos: 0,
                    ..Default::default()
                }),
                ..original.clone()
            },
            ActivationFactsError::Validity,
        ),
        (
            CredentialActivation {
                not_before: MessageField::some(Timestamp {
                    seconds: 1_800_000_000,
                    nanos: -1,
                    ..Default::default()
                }),
                ..original.clone()
            },
            ActivationFactsError::Validity,
        ),
    ];
    for (message, expected) in invalid {
        assert_eq!(CredentialActivationFacts::try_from(message), Err(expected));
    }
}

#[test]
fn activation_binding_refuses_role_and_identity_confusion() {
    let mut message = activation();
    for binding in [
        RegistrationBinding {
            role: EnumValue::from(WorkloadRole::Agent),
            application_id: None,
            ..(*message.binding).clone()
        },
        RegistrationBinding {
            role: EnumValue::from(WorkloadRole::Unspecified),
            ..(*message.binding).clone()
        },
        RegistrationBinding {
            role: EnumValue::from(999),
            ..(*message.binding).clone()
        },
        RegistrationBinding {
            application_id: Some("unexpected-agent".into()),
            ..(*message.binding).clone()
        },
        RegistrationBinding {
            spiffe_id: "https://example.test/proxy".into(),
            ..(*message.binding).clone()
        },
        RegistrationBinding {
            tenant: "another tenant".into(),
            ..(*message.binding).clone()
        },
        RegistrationBinding {
            project: String::new(),
            ..(*message.binding).clone()
        },
    ] {
        message.binding = MessageField::some(binding);
        assert_eq!(
            CredentialActivationFacts::try_from(message.clone()),
            Err(ActivationFactsError::Binding)
        );
    }
}

#[tokio::test]
async fn activation_recorder_waits_for_a_durable_acknowledgement() {
    let sink = MemorySink::new();
    sink.hold();
    let writer = AuditWriter::new(sink.clone(), identity(), WriterConfig::default(), &stamper());
    let recorder = ActivationRecorder::new(writer, scope());
    let facts = CredentialActivationFacts::try_from(activation()).unwrap();
    let record = recorder.record(facts);
    tokio::pin!(record);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(10), &mut record)
            .await
            .is_err()
    );
    assert!(sink.stored().is_empty());
    sink.release();
    record.await.unwrap();
    assert_eq!(sink.stored().len(), 2);
}

#[test]
fn activation_records_preserve_registered_application_identity_independently_of_transport_path() {
    let mut message = activation();
    message.binding.as_option_mut().unwrap().role = EnumValue::from(WorkloadRole::Agent);
    message.binding.as_option_mut().unwrap().spiffe_id = "spiffe://example.test/opaque/deployment/7".into();
    message.binding.as_option_mut().unwrap().application_id = Some("did:key:zExample".into());
    let facts = CredentialActivationFacts::try_from(message.clone()).unwrap();
    let record = Record::credential_activation(scope(), facts, stamper().stamp()).unwrap();
    let Some(Detail::CredentialActivation(actual)) = record.message().detail.as_ref() else {
        panic!("activation detail");
    };
    assert_eq!(actual.binding.application_id.as_deref(), Some("did:key:zExample"));
    message.binding.as_option_mut().unwrap().spiffe_id = "spiffe://example.test".into();
    assert!(CredentialActivationFacts::try_from(message).is_ok());
}

#[tokio::test]
async fn an_unrecorded_activation_never_returns_a_receipt() {
    let sink = MemorySink::new();
    sink.fail_all(SinkError::Timeout);
    let writer = AuditWriter::new(sink.clone(), identity(), WriterConfig::default(), &stamper());
    let recorder = ActivationRecorder::new(writer.clone(), scope());
    let facts = CredentialActivationFacts::try_from(activation()).unwrap();
    assert!(matches!(
        recorder.record(facts.clone()).await,
        Err(ActivationRecorderError::Unavailable(_))
    ));
    assert!(sink.stored().is_empty());
    sink.recover();
    writer.drain_once().await;
    recorder.record(facts).await.unwrap();
    let verified = verify_chain_range(
        ChainAnchor::Genesis {
            writer: identity().writer().clone(),
            chain: writer.chain(),
        },
        sink.stored_records(),
    )
    .unwrap();
    assert_eq!(verified.head().sequence(), 2);
}

#[tokio::test]
async fn activation_scope_mismatch_is_refused_before_any_audit_write() {
    let sink = MemorySink::new();
    let writer = AuditWriter::new(sink.clone(), identity(), WriterConfig::default(), &stamper());
    let other_scope = RecordScope::new(name("tenant_2"), token("project_1"), Actor::Unattributed);
    let recorder = ActivationRecorder::new(writer, other_scope);
    let facts = CredentialActivationFacts::try_from(activation()).unwrap();
    assert!(matches!(
        recorder.record(facts).await,
        Err(ActivationRecorderError::Scope(_))
    ));
    assert_eq!(sink.attempts(), 0);
}

#[tokio::test]
async fn offline_verification_refuses_resealed_activation_authority_confusion() {
    let sink = MemorySink::new();
    let writer = AuditWriter::new(sink.clone(), identity(), WriterConfig::default(), &stamper());
    let recorder = ActivationRecorder::new(writer.clone(), scope());
    recorder
        .record(CredentialActivationFacts::try_from(activation()).unwrap())
        .await
        .unwrap();
    let anchor = ChainAnchor::Genesis {
        writer: identity().writer().clone(),
        chain: writer.chain(),
    };
    let records = sink.stored_records();
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
    message.attribution.as_option_mut().unwrap().would_deny = true;
    tampered.push(message);
    let mut message = original.clone();
    message.attribution = MessageField::none();
    tampered.push(message);
    let mut message = original.clone();
    let Some(Detail::CredentialActivation(activation)) = message.detail.as_mut() else {
        panic!("activation detail");
    };
    activation.bundle_sha256.pop();
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
fn activation_facts_reject_unknown_timestamp_bytes() {
    let mut message = activation();
    let mut bytes = message.expires_at.encode_to_vec();
    bytes.extend_from_slice(&[0x7a, 1, b'A']);
    let timestamp = Timestamp::decode_from_slice(&bytes).unwrap();
    assert!(!timestamp.__buffa_unknown_fields.is_empty());
    message.expires_at = MessageField::some(timestamp);
    assert_eq!(CredentialActivationFacts::try_from(message), Err(ActivationFactsError::Validity));
}
