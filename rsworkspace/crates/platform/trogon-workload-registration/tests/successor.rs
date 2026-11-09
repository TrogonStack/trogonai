#![allow(clippy::expect_used)]

use std::num::NonZeroU64;
use std::time::{Duration, SystemTime};

use buffa::{EnumValue, Message, MessageField};
use buffa_types::google::protobuf::Timestamp;
use ed25519_dalek::{Signer, SigningKey};
use trogon_workload_registration::{ApprovedRegistration, CanonicalAgentId, ProducerConfig, approval_signing_bytes};
use trogonai_proto::platform::workload_identity::v1alpha1::{RegistrationBinding, RegistrationSnapshot, WorkloadRole};

fn snapshot(issued_at: SystemTime, revision: u64, trust: u64, root: u8) -> RegistrationSnapshot {
    RegistrationSnapshot {
        tenant: "acme".into(),
        project: "payments".into(),
        recovery_fence: 1,
        registration_revision: revision,
        trust_revision: trust,
        issued_at: MessageField::some(Timestamp::from(issued_at)),
        expires_at: MessageField::some(Timestamp::from(issued_at + Duration::from_secs(30))),
        bindings: vec![RegistrationBinding {
            spiffe_id: "spiffe://identity.example.org/service/egress".into(),
            tenant: "acme".into(),
            project: "payments".into(),
            role: EnumValue::from(WorkloadRole::EgressProxy),
            application_id: None,
        }],
        approved_root_sha256: vec![vec![root; 32]],
    }
}

fn approved(message: &RegistrationSnapshot) -> ApprovedRegistration {
    let key = SigningKey::from_bytes(&[7; 32]);
    let config = ProducerConfig::new(
        "acme",
        "payments",
        "identity.example.org",
        NonZeroU64::MIN,
        key.verifying_key(),
    )
    .expect("producer configuration");
    let payload = message.encode_to_vec();
    let signature = key.sign(&approval_signing_bytes(&config, &payload)).to_bytes();
    ApprovedRegistration::verify(&config, &payload, &signature).expect("independent fresh canonical approval")
}

#[test]
fn every_renewal_advances_registration_revision_for_exact_reader_compatibility() {
    let now = SystemTime::now() - Duration::from_secs(2);
    let previous = approved(&snapshot(now, 1, 1, 1));
    let renewed = approved(&snapshot(now + Duration::from_secs(1), 1, 1, 1));
    assert!(
        renewed.check_successor(&previous).is_err(),
        "equal-version changed expiry equivocates at the runtime reader"
    );
}

#[test]
fn renewal_preserves_trust_revision_only_for_identical_roots() {
    let now = SystemTime::now() - Duration::from_secs(2);
    let previous = approved(&snapshot(now, 1, 1, 1));
    let renewed = approved(&snapshot(now + Duration::from_secs(1), 2, 1, 1));
    assert!(renewed.check_successor(&previous).is_ok());
    let equivocal_roots = approved(&snapshot(now + Duration::from_secs(1), 2, 1, 2));
    assert!(equivocal_roots.check_successor(&previous).is_err());
    let rotated_roots = approved(&snapshot(now + Duration::from_secs(1), 2, 2, 2));
    assert!(rotated_roots.check_successor(&previous).is_ok());
    assert!(previous.check_successor(&renewed).is_err());
}

#[test]
fn a_real_ed25519_application_key_is_accepted_without_using_the_transport_path() {
    let public = SigningKey::from_bytes(&[9; 32]).verifying_key();
    let identity = format!(
        "did:key:z{}",
        bs58::encode([&[0xed, 0x01][..], public.as_bytes()].concat()).into_string()
    );
    assert_eq!(
        CanonicalAgentId::new(&identity)
            .expect("canonical application identity")
            .as_str(),
        identity
    );
    let mut message = snapshot(SystemTime::now(), 1, 1, 1);
    message.bindings[0].role = EnumValue::from(WorkloadRole::Agent);
    message.bindings[0].application_id = Some(identity.clone());
    let registration = approved(&message);
    assert_eq!(
        registration.bindings()[0]
            .application_id()
            .expect("governed application identity")
            .as_str(),
        identity
    );
}

#[test]
fn a_valid_signature_does_not_bypass_protocol_binding_bounds() {
    let key = SigningKey::from_bytes(&[7; 32]);
    let config = ProducerConfig::new(
        "acme",
        "payments",
        "identity.example.org",
        NonZeroU64::MIN,
        key.verifying_key(),
    )
    .expect("producer configuration");
    let mut message = snapshot(SystemTime::now(), 1, 1, 1);
    message.bindings[0].spiffe_id = format!("spiffe://identity.example.org/{}", "x".repeat(2048));
    let payload = message.encode_to_vec();
    let signature = key.sign(&approval_signing_bytes(&config, &payload)).to_bytes();
    assert!(ApprovedRegistration::verify(&config, &payload, &signature).is_err());
}

#[test]
fn all_trust_withdrawal_remains_a_valid_signed_revision_fence() {
    let now = SystemTime::now() - Duration::from_secs(2);
    let previous = approved(&snapshot(now, 1, 1, 1));
    let mut message = snapshot(now + Duration::from_secs(1), 2, 100, 1);
    message.approved_root_sha256.clear();
    let withdrawal = approved(&message);
    assert!(withdrawal.roots().is_empty());
    assert!(withdrawal.check_successor(&previous).is_ok());
    let retired_roots = approved(&snapshot(now + Duration::from_secs(2), 500, 50, 1));
    assert!(retired_roots.check_successor(&withdrawal).is_err());
    let mut restored = snapshot(now + Duration::from_secs(2), 3, 101, 2);
    restored.issued_at = MessageField::some(Timestamp::from(SystemTime::now()));
    restored.expires_at = MessageField::some(Timestamp::from(SystemTime::now() + Duration::from_secs(30)));
    assert!(approved(&restored).check_successor(&withdrawal).is_ok());
}
