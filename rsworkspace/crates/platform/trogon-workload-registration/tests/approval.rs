#![allow(clippy::expect_used)]

use std::num::NonZeroU64;
use std::time::{Duration, SystemTime};

use buffa::{EnumValue, Message, MessageField};
use buffa_types::google::protobuf::Timestamp;
use ed25519_dalek::{Signer, SigningKey};
use trogon_workload_registration::{ApprovedRegistration, ProducerConfig, SIGNING_CONTEXT};
use trogonai_proto::platform::workload_identity::v1alpha1::{RegistrationBinding, RegistrationSnapshot, WorkloadRole};

fn snapshot() -> RegistrationSnapshot {
    let now = SystemTime::now();
    RegistrationSnapshot {
        tenant: "acme".into(),
        project: "payments".into(),
        recovery_fence: 1,
        registration_revision: 1,
        trust_revision: 1,
        issued_at: MessageField::some(Timestamp::from(now)),
        expires_at: MessageField::some(Timestamp::from(now + Duration::from_secs(30))),
        bindings: vec![RegistrationBinding {
            spiffe_id: "spiffe://identity.example.org/service/egress".into(),
            tenant: "acme".into(),
            project: "payments".into(),
            role: EnumValue::from(WorkloadRole::EgressProxy),
            application_id: None,
        }],
        approved_root_sha256: vec![vec![1; 32]],
    }
}

fn config(key: &SigningKey) -> ProducerConfig {
    ProducerConfig::new(
        "acme",
        "payments",
        "identity.example.org",
        NonZeroU64::MIN,
        key.verifying_key(),
    )
    .expect("producer configuration")
}

fn signature(key: &SigningKey, payload: &[u8]) -> [u8; 64] {
    key.sign(&[SIGNING_CONTEXT, payload].concat()).to_bytes()
}

#[test]
fn only_fresh_exact_scoped_and_independently_signed_snapshots_are_authority() {
    let key = SigningKey::from_bytes(&[7; 32]);
    let config = config(&key);
    let valid = snapshot().encode_to_vec();
    assert!(ApprovedRegistration::verify(&config, &valid, &signature(&key, &valid)).is_ok());
    assert!(ApprovedRegistration::verify(&config, &valid, &[0; 64]).is_err());
    let other = SigningKey::from_bytes(&[8; 32]);
    assert!(ApprovedRegistration::verify(&config, &valid, &signature(&other, &valid)).is_err());
    assert!(ApprovedRegistration::verify(&config, &valid, &key.sign(&valid).to_bytes()).is_err());

    for changed in [
        RegistrationSnapshot {
            tenant: "another".into(),
            ..snapshot()
        },
        RegistrationSnapshot {
            recovery_fence: 2,
            ..snapshot()
        },
        RegistrationSnapshot {
            registration_revision: 0,
            ..snapshot()
        },
        RegistrationSnapshot {
            trust_revision: 0,
            ..snapshot()
        },
        RegistrationSnapshot {
            approved_root_sha256: vec![vec![1; 31]],
            ..snapshot()
        },
        RegistrationSnapshot {
            approved_root_sha256: vec![vec![1; 32], vec![1; 32]],
            ..snapshot()
        },
        RegistrationSnapshot {
            issued_at: MessageField::some(Timestamp::from(SystemTime::now() + Duration::from_secs(30))),
            ..snapshot()
        },
        RegistrationSnapshot {
            expires_at: MessageField::some(Timestamp::from(SystemTime::now() + Duration::from_secs(331))),
            ..snapshot()
        },
        RegistrationSnapshot {
            expires_at: MessageField::some(Timestamp::from(SystemTime::now() - Duration::from_secs(1))),
            ..snapshot()
        },
        RegistrationSnapshot {
            bindings: vec![snapshot().bindings[0].clone(); 2],
            ..snapshot()
        },
    ] {
        let payload = changed.encode_to_vec();
        assert!(ApprovedRegistration::verify(&config, &payload, &signature(&key, &payload)).is_err());
    }

    let mut unknown = valid.clone();
    unknown.extend_from_slice(&[0xa0, 0x06, 0x01]);
    assert!(ApprovedRegistration::verify(&config, &unknown, &signature(&key, &unknown)).is_err());
    let mut withdrawal = snapshot();
    withdrawal.bindings.clear();
    let payload = withdrawal.encode_to_vec();
    assert!(ApprovedRegistration::verify(&config, &payload, &signature(&key, &payload)).is_ok());

    for (identity, tenant, project, role, application) in [
        (
            "spiffe://elsewhere.example.org/service/egress",
            "acme",
            "payments",
            WorkloadRole::EgressProxy,
            None,
        ),
        (
            "spiffe://identity.example.org/service/egress",
            "another",
            "payments",
            WorkloadRole::EgressProxy,
            None,
        ),
        (
            "spiffe://identity.example.org/service/egress",
            "acme",
            "another",
            WorkloadRole::EgressProxy,
            None,
        ),
        (
            "spiffe://identity.example.org/runtime/agent",
            "acme",
            "payments",
            WorkloadRole::Agent,
            Some("agent-1"),
        ),
        (
            "spiffe://identity.example.org/service/egress",
            "acme",
            "payments",
            WorkloadRole::EgressProxy,
            Some("agent-1"),
        ),
    ] {
        let mut changed = snapshot();
        changed.bindings = vec![RegistrationBinding {
            spiffe_id: identity.into(),
            tenant: tenant.into(),
            project: project.into(),
            role: EnumValue::from(role),
            application_id: application.map(str::to_owned),
        }];
        let payload = changed.encode_to_vec();
        assert!(ApprovedRegistration::verify(&config, &payload, &signature(&key, &payload)).is_err());
    }
}

#[test]
fn a_signing_key_does_not_remove_scope_configuration_constraints() {
    let key = SigningKey::from_bytes(&[7; 32]).verifying_key();
    for (tenant, project, domain) in [
        ("", "payments", "identity.example.org"),
        ("acme.*", "payments", "identity.example.org"),
        ("acme", "payments.*", "identity.example.org"),
        ("acme", "payments", "identity.example.org/other"),
    ] {
        assert!(ProducerConfig::new(tenant, project, domain, NonZeroU64::MIN, key).is_err());
    }
    let mut identity_point = [0; 32];
    identity_point[0] = 1;
    let weak = ed25519_dalek::VerifyingKey::from_bytes(&identity_point).expect("encoded identity point");
    assert!(ProducerConfig::new("acme", "payments", "identity.example.org", NonZeroU64::MIN, weak).is_err());
}
