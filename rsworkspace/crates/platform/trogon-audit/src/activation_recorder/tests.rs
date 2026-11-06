use buffa::{EnumValue, MessageField};
use buffa_types::google::protobuf::Timestamp;
use trogonai_proto::platform::audit::v1::AuditClass;
use trogonai_proto::platform::audit::v1::audit_record::Detail;
use trogonai_proto::platform::workload_identity::v1alpha1::{CredentialActivation, RegistrationBinding, WorkloadRole};

use crate::fixtures::{identity, scope, stamper};
use crate::{
    ActivationFactsError, ActivationRecorder, AuditWriter, CredentialActivationFacts, MemorySink, Obligation, Record,
    WriterConfig,
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
        (CredentialActivation { binding: MessageField::none(), ..original.clone() }, ActivationFactsError::Binding),
        (CredentialActivation { source_revision: 0, ..original.clone() }, ActivationFactsError::Generation),
        (CredentialActivation { recovery_fence: 0, ..original.clone() }, ActivationFactsError::Generation),
        (CredentialActivation { registration_revision: 0, ..original.clone() }, ActivationFactsError::Generation),
        (CredentialActivation { trust_revision: 0, ..original.clone() }, ActivationFactsError::Generation),
        (CredentialActivation { activation_revision: 0, ..original.clone() }, ActivationFactsError::Generation),
        (CredentialActivation { bundle_sha256: vec![1; 33], ..original.clone() }, ActivationFactsError::Fingerprint),
        (CredentialActivation { leaf_sha256: vec![], ..original.clone() }, ActivationFactsError::Fingerprint),
        (CredentialActivation { not_before: MessageField::none(), ..original.clone() }, ActivationFactsError::Validity),
        (CredentialActivation { expires_at: MessageField::none(), ..original.clone() }, ActivationFactsError::Validity),
        (CredentialActivation { expires_at: original.not_before.clone(), ..original.clone() }, ActivationFactsError::Validity),
        (CredentialActivation { expires_at: MessageField::some(Timestamp { seconds: 1_800_003_601, nanos: 0, ..Default::default() }), ..original.clone() }, ActivationFactsError::Validity),
        (CredentialActivation { not_before: MessageField::some(Timestamp { seconds: 1_800_000_000, nanos: -1, ..Default::default() }), ..original.clone() }, ActivationFactsError::Validity),
    ];
    for (message, expected) in invalid {
        assert_eq!(CredentialActivationFacts::try_from(message), Err(expected));
    }
}

#[test]
fn activation_binding_refuses_role_and_identity_confusion() {
    let mut message = activation();
    for binding in [
        RegistrationBinding { role: EnumValue::from(WorkloadRole::Agent), application_id: None, ..(*message.binding).clone() },
        RegistrationBinding { role: EnumValue::from(WorkloadRole::Unspecified), ..(*message.binding).clone() },
        RegistrationBinding { role: EnumValue::from(999), ..(*message.binding).clone() },
        RegistrationBinding { application_id: Some("unexpected-agent".into()), ..(*message.binding).clone() },
        RegistrationBinding { spiffe_id: "https://example.test/proxy".into(), ..(*message.binding).clone() },
        RegistrationBinding { tenant: "another tenant".into(), ..(*message.binding).clone() },
        RegistrationBinding { project: String::new(), ..(*message.binding).clone() },
    ] {
        message.binding = MessageField::some(binding);
        assert_eq!(CredentialActivationFacts::try_from(message.clone()), Err(ActivationFactsError::Binding));
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
