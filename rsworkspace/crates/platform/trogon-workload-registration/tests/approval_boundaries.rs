#![allow(clippy::expect_used)]

use std::num::NonZeroU64;
use std::time::{Duration, SystemTime};

use buffa::{EnumValue, Message, MessageField};
use buffa_types::google::protobuf::Timestamp;
use ed25519_dalek::{Signer, SigningKey};
use trogon_audit::{AuditName, AuditWriter, MemorySink, Stamper, WriterConfig, WriterId, WriterIdentity};
use trogon_nats::{NatsConfig, NatsTls, NatsToken};
use trogon_std::{SystemClock, UuidV7Generator};
use trogon_workload_identity::constants::{MAX_BINDINGS, MAX_ROOTS};
use trogon_workload_registration::{
    ApprovedRegistration, CanonicalAgentId, MAX_SNAPSHOT_BYTES, ProducerConfig, ProductionRegistrationProducer,
    RegistrationError, RegistrationProducer, approval_signing_bytes, emergency_withdrawal,
    production_emergency_withdrawal, provision_streams,
};
use trogonai_proto::platform::workload_identity::v1alpha1::{RegistrationBinding, RegistrationSnapshot, WorkloadRole};

fn config(fence: NonZeroU64) -> ProducerConfig {
    ProducerConfig::new(
        "acme",
        "payments",
        "identity.example.org",
        fence,
        SigningKey::from_bytes(&[7; 32]).verifying_key(),
    )
    .expect("scope")
}

fn snapshot() -> RegistrationSnapshot {
    let issued = SystemTime::now() - Duration::from_secs(2);
    RegistrationSnapshot {
        tenant: "acme".into(),
        project: "payments".into(),
        recovery_fence: 1,
        registration_revision: 1,
        trust_revision: 1,
        issued_at: MessageField::some(Timestamp::from(issued)),
        expires_at: MessageField::some(Timestamp::from(issued + Duration::from_secs(120))),
        bindings: vec![RegistrationBinding {
            spiffe_id: "spiffe://identity.example.org/service/supervisor".into(),
            tenant: "acme".into(),
            project: "payments".into(),
            role: EnumValue::from(WorkloadRole::Supervisor),
            application_id: None,
        }],
        approved_root_sha256: vec![vec![1; 32]],
    }
}

fn verify(snapshot: &RegistrationSnapshot) -> Result<ApprovedRegistration, RegistrationError> {
    let config = config(NonZeroU64::MIN);
    let payload = snapshot.encode_to_vec();
    let signature = SigningKey::from_bytes(&[7; 32])
        .sign(&approval_signing_bytes(&config, &payload))
        .to_bytes();
    ApprovedRegistration::verify(&config, &payload, &signature)
}

fn audit() -> AuditWriter<MemorySink> {
    AuditWriter::new(
        MemorySink::new(),
        WriterIdentity::new(
            WriterId::new("approval-boundary").expect("writer"),
            NatsToken::new("registration").expect("component"),
            AuditName::new("acme").expect("tenant"),
            NatsToken::new("payments").expect("project"),
        ),
        WriterConfig::default(),
        &Stamper::new(SystemClock, UuidV7Generator),
    )
}

#[test]
fn signatures_do_not_bypass_resource_timestamp_or_role_contracts() {
    let config = config(NonZeroU64::MIN);
    assert!(matches!(
        ApprovedRegistration::verify(&config, &vec![0; MAX_SNAPSHOT_BYTES + 1], &[0; 64]),
        Err(RegistrationError::Bounds)
    ));
    let supervisor_snapshot = snapshot();
    let supervisor = verify(&supervisor_snapshot).expect("explicit governed supervisor role");
    assert_eq!(
        supervisor.bindings()[0].role(),
        trogon_workload_identity::WorkloadRole::Supervisor
    );
    assert_eq!(
        supervisor.payload().as_ref(),
        supervisor_snapshot.encode_to_vec().as_slice()
    );
    let mut invalid = snapshot();
    invalid.bindings = vec![invalid.bindings[0].clone(); MAX_BINDINGS + 1];
    assert!(matches!(verify(&invalid), Err(RegistrationError::Bounds)));
    invalid = snapshot();
    invalid.approved_root_sha256 = vec![vec![1; 32]; MAX_ROOTS + 1];
    assert!(matches!(verify(&invalid), Err(RegistrationError::Bounds)));
    invalid = snapshot();
    invalid.bindings[0].role = EnumValue::from(WorkloadRole::Unspecified);
    assert!(matches!(verify(&invalid), Err(RegistrationError::Scope)));
    for timestamp in [
        Timestamp {
            seconds: 0,
            nanos: 0,
            ..Timestamp::default()
        },
        Timestamp {
            seconds: 1,
            nanos: -1,
            ..Timestamp::default()
        },
        Timestamp {
            seconds: 1,
            nanos: 1_000_000_000,
            ..Timestamp::default()
        },
    ] {
        invalid = snapshot();
        invalid.issued_at = MessageField::some(timestamp);
        assert!(matches!(verify(&invalid), Err(RegistrationError::Lease)));
    }
}

#[test]
fn approval_diagnostics_preserve_boundary_order_and_canonical_collections() {
    let mut invalid = snapshot();
    invalid.tenant = "other".into();
    invalid.issued_at = MessageField::none();
    let payload = invalid.encode_to_vec();
    assert!(matches!(
        ApprovedRegistration::verify(&config(NonZeroU64::MIN), &payload, &[0; 64]),
        Err(RegistrationError::Signature(_))
    ));
    assert!(matches!(verify(&invalid), Err(RegistrationError::Scope)));

    invalid = snapshot();
    invalid.registration_revision = 0;
    invalid.issued_at = MessageField::none();
    assert!(matches!(verify(&invalid), Err(RegistrationError::Revision)));

    invalid = snapshot();
    invalid.expires_at = MessageField::some(Timestamp::from(SystemTime::now() - Duration::from_secs(1)));
    invalid.bindings = vec![invalid.bindings[0].clone(); MAX_BINDINGS + 1];
    assert!(matches!(verify(&invalid), Err(RegistrationError::Lease)));

    invalid = snapshot();
    invalid.expires_at = MessageField::some(Timestamp::from(SystemTime::now() + Duration::from_secs(331)));
    invalid.bindings[0].role = EnumValue::from(WorkloadRole::Unspecified);
    assert!(matches!(verify(&invalid), Err(RegistrationError::Lease)));

    invalid = snapshot();
    invalid.bindings.push(invalid.bindings[0].clone());
    assert!(matches!(verify(&invalid), Err(RegistrationError::Bounds)));

    invalid = snapshot();
    invalid.bindings.push(RegistrationBinding {
        spiffe_id: "spiffe://identity.example.org/service/alpha".into(),
        ..invalid.bindings[0].clone()
    });
    assert!(matches!(verify(&invalid), Err(RegistrationError::NoncanonicalPayload)));

    invalid = snapshot();
    invalid.approved_root_sha256 = vec![vec![2; 32], vec![1; 32]];
    assert!(matches!(verify(&invalid), Err(RegistrationError::NoncanonicalPayload)));
}

#[test]
fn configured_domain_and_application_identity_cannot_be_normalized_into_authority() {
    let key = SigningKey::from_bytes(&[7; 32]).verifying_key();
    for alias in ["spiffe://identity.example.org/path", "IDENTITY.EXAMPLE.ORG"] {
        assert!(matches!(
            ProducerConfig::new("acme", "payments", alias, NonZeroU64::MIN, key),
            Err(RegistrationError::NoncanonicalDomain)
        ));
    }
    assert!(matches!(
        CanonicalAgentId::new(&"x".repeat(129)),
        Err(RegistrationError::ApplicationIdentity)
    ));
    let wrong_codec = format!("did:key:z{}", bs58::encode([0; 34]).into_string());
    assert!(matches!(
        CanonicalAgentId::new(&wrong_codec),
        Err(RegistrationError::ApplicationIdentity)
    ));
    let mut weak = [0; 34];
    weak[..2].copy_from_slice(&[0xed, 0x01]);
    weak[2] = 1;
    let weak = format!("did:key:z{}", bs58::encode(weak).into_string());
    assert!(matches!(
        CanonicalAgentId::new(&weak),
        Err(RegistrationError::ApplicationIdentity)
    ));
}

#[tokio::test]
async fn approval_held_past_its_original_deadline_is_not_reusable() {
    let mut expiring = snapshot();
    expiring.expires_at = MessageField::some(Timestamp::from(SystemTime::now() + Duration::from_secs(1)));
    let approval = verify(&expiring).expect("fresh finite approval");
    let deadline = approval.lease().deadline();
    tokio::time::sleep_until((deadline + Duration::from_millis(1)).into()).await;
    assert!(matches!(approval.check_current(), Err(RegistrationError::Lease)));
    assert_eq!(approval.lease().deadline(), deadline);
    let inaccessible = NatsConfig::from_url("127.0.0.1:1").with_tls(NatsTls::Disabled);
    let other = config(NonZeroU64::new(2).expect("other fence"));
    assert!(matches!(
        emergency_withdrawal(&inaccessible, &other, approval.clone(), &audit()).await,
        Err(RegistrationError::Lease)
    ));
    assert!(matches!(
        production_emergency_withdrawal(&inaccessible, &other, approval).await,
        Err(RegistrationError::Lease)
    ));
}

#[tokio::test]
async fn wrong_scope_non_denials_and_insecure_transport_fail_before_broker_access() {
    let current = config(NonZeroU64::MIN);
    let other = config(NonZeroU64::new(2).expect("independent fence"));
    let grant = verify(&snapshot()).expect("grant");
    let inaccessible = NatsConfig::from_url("127.0.0.1:1").with_tls(NatsTls::Disabled);
    assert!(matches!(
        ProductionRegistrationProducer::start(other.clone(), inaccessible.clone(), grant.clone()).await,
        Err(RegistrationError::Scope)
    ));
    assert!(matches!(
        RegistrationProducer::claim(other.clone(), inaccessible.clone(), grant.clone(), audit()).await,
        Err(RegistrationError::Scope)
    ));
    assert!(matches!(
        emergency_withdrawal(&inaccessible, &other, grant.clone(), &audit()).await,
        Err(RegistrationError::Scope)
    ));
    assert!(matches!(
        production_emergency_withdrawal(&inaccessible, &other, grant.clone()).await,
        Err(RegistrationError::Scope)
    ));
    assert!(matches!(
        emergency_withdrawal(&inaccessible, &current, grant.clone(), &audit()).await,
        Err(RegistrationError::NotWithdrawal)
    ));
    assert!(matches!(
        production_emergency_withdrawal(&inaccessible, &current, grant.clone()).await,
        Err(RegistrationError::NotWithdrawal)
    ));
    assert!(matches!(
        RegistrationProducer::claim(current.clone(), inaccessible.clone(), grant, audit()).await,
        Err(RegistrationError::Transport)
    ));
    assert!(matches!(
        provision_streams(&inaccessible, &current).await,
        Err(RegistrationError::Transport)
    ));
}
