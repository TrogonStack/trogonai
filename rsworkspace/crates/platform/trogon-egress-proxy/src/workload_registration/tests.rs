use std::num::NonZeroU64;
use std::time::{Duration, SystemTime};

use buffa::{EnumValue, Message, MessageField};
use buffa_types::google::protobuf::Timestamp;
use trogon_workload_identity::{IdentityConfig, RegistrationBinding, UnixEndpoint, WorkloadRole};
use trogonai_proto::platform::workload_identity::v1alpha1::{
    RegistrationBinding as WireBinding, RegistrationSnapshot, WorkloadRole as WireRole,
};

use super::*;

fn config() -> IdentityConfig {
    IdentityConfig::new(
        UnixEndpoint::new("unix:///run/identity/api.sock").expect("endpoint"),
        RegistrationBinding::new(
            "spiffe://trogon.example/service/egress",
            "acme",
            "payments",
            WorkloadRole::EgressProxy,
            None,
        )
        .expect("binding"),
        NonZeroU64::MIN,
    )
}

fn snapshot(now: SystemTime) -> RegistrationSnapshot {
    RegistrationSnapshot {
        tenant: "acme".into(),
        project: "payments".into(),
        recovery_fence: 1,
        registration_revision: 2,
        trust_revision: 3,
        issued_at: MessageField::some(Timestamp::from(now)),
        expires_at: MessageField::some(Timestamp::from(now + Duration::from_secs(30))),
        bindings: vec![WireBinding {
            spiffe_id: "spiffe://trogon.example/service/egress".into(),
            tenant: "acme".into(),
            project: "payments".into(),
            role: EnumValue::from(WireRole::EgressProxy),
            application_id: None,
        }],
        approved_root_sha256: vec![vec![1; 32]],
    }
}

#[test]
fn binary_registration_projection_preserves_exact_governed_authority() {
    let now = SystemTime::now();
    let message = snapshot(now);
    let registration = decode_registration(&message.encode_to_vec(), &config(), now).expect("valid projection");
    assert_eq!(registration.recovery_fence().get(), 1);
    assert_eq!(registration.registration_revision().get(), 2);
    assert_eq!(registration.trust_revision().get(), 3);
    assert_eq!(registration.approved_roots(), &[[1; 32]]);
    assert_eq!(registration.bindings(), &[config().local().clone()]);
}

#[test]
fn scoped_revocation_projection_preserves_its_revision_without_a_local_grant() {
    let now = SystemTime::now();
    let mut message = snapshot(now);
    message.registration_revision = 100;
    message.bindings = vec![WireBinding {
        spiffe_id: "spiffe://trogon.example/runtime/peer".into(),
        tenant: "acme".into(),
        project: "payments".into(),
        role: EnumValue::from(WireRole::Agent),
        application_id: Some("agent-1".into()),
    }];
    let registration = decode_registration(&message.encode_to_vec(), &config(), now)
        .expect("a valid revocation must reach the generation fence");
    assert_eq!(registration.registration_revision().get(), 100);
    assert!(!registration.bindings().contains(config().local()));
}

#[test]
fn scoped_empty_revocation_projection_preserves_its_revision() {
    let now = SystemTime::now();
    let mut message = snapshot(now);
    message.registration_revision = 100;
    message.bindings.clear();
    let registration = decode_registration(&message.encode_to_vec(), &config(), now)
        .expect("an empty governed snapshot revokes every local grant");
    assert_eq!(registration.registration_revision().get(), 100);
    assert!(registration.bindings().is_empty());
}

#[test]
fn scoped_empty_trust_projection_preserves_its_trust_floor_without_authorizing_roots() {
    let now = SystemTime::now();
    let mut message = snapshot(now);
    message.registration_revision = 100;
    message.trust_revision = 100;
    message.approved_root_sha256.clear();
    let registration = decode_registration(&message.encode_to_vec(), &config(), now)
        .expect("complete trust withdrawal must reach the generation fence");
    assert_eq!(registration.registration_revision().get(), 100);
    assert_eq!(registration.trust_revision().get(), 100);
    assert!(registration.approved_roots().is_empty());
    assert_eq!(registration.bindings(), &[config().local().clone()]);
}

#[test]
fn application_identity_comes_from_registration_instead_of_the_spiffe_path() {
    let now = SystemTime::now();
    let mut message = snapshot(now);
    message.bindings.push(WireBinding {
        spiffe_id: "spiffe://trogon.example/runtime/opaque-selector".into(),
        tenant: "acme".into(),
        project: "payments".into(),
        role: EnumValue::from(WireRole::Agent),
        application_id: Some("agent-1".into()),
    });
    let registration = decode_registration(&message.encode_to_vec(), &config(), now).expect("projection");
    let agent = registration
        .bindings()
        .iter()
        .find(|binding| binding.role() == WorkloadRole::Agent)
        .expect("agent");
    assert_eq!(agent.application_id().expect("application").as_str(), "agent-1");
    assert_eq!(
        agent.spiffe_id().to_string(),
        "spiffe://trogon.example/runtime/opaque-selector"
    );
}

#[test]
fn wire_scope_and_recovery_cannot_replace_the_configured_authority() {
    let now = SystemTime::now();
    let original = snapshot(now);
    for message in [
        RegistrationSnapshot {
            tenant: "other".into(),
            ..original.clone()
        },
        RegistrationSnapshot {
            project: "other".into(),
            ..original.clone()
        },
        RegistrationSnapshot {
            recovery_fence: 2,
            ..original.clone()
        },
    ] {
        assert!(matches!(
            decode_registration(&message.encode_to_vec(), &config(), now),
            Err(RegistrationProjectionError::Scope)
        ));
    }
    for binding in [
        WireBinding {
            tenant: "other".into(),
            ..original.bindings[0].clone()
        },
        WireBinding {
            spiffe_id: "spiffe://other.example/service/egress".into(),
            ..original.bindings[0].clone()
        },
    ] {
        let mut message = original.clone();
        message.bindings[0] = binding;
        assert!(decode_registration(&message.encode_to_vec(), &config(), now).is_err());
    }
}

#[test]
fn inert_timestamp_annotations_cannot_grant_a_future_or_extended_lease() {
    let now = SystemTime::now();
    let original = snapshot(now);
    for message in [
        RegistrationSnapshot {
            issued_at: MessageField::none(),
            ..original.clone()
        },
        RegistrationSnapshot {
            expires_at: MessageField::none(),
            ..original.clone()
        },
        RegistrationSnapshot {
            issued_at: MessageField::some(Timestamp::from(now + Duration::from_nanos(1))),
            ..original.clone()
        },
        RegistrationSnapshot {
            expires_at: MessageField::some(Timestamp::from(now)),
            ..original.clone()
        },
        RegistrationSnapshot {
            expires_at: MessageField::some(Timestamp::from(now + Duration::from_secs(331))),
            ..original.clone()
        },
        RegistrationSnapshot {
            issued_at: MessageField::some(Timestamp {
                seconds: 1,
                nanos: -1,
                ..Default::default()
            }),
            ..original.clone()
        },
        RegistrationSnapshot {
            expires_at: MessageField::some(Timestamp {
                seconds: i64::MAX,
                nanos: 0,
                ..Default::default()
            }),
            ..original.clone()
        },
    ] {
        assert!(matches!(
            decode_registration(&message.encode_to_vec(), &config(), now),
            Err(RegistrationProjectionError::Validity)
        ));
    }
}

#[test]
fn malformed_or_ambiguous_projection_values_are_refused() {
    let now = SystemTime::now();
    let original = snapshot(now);
    for message in [
        RegistrationSnapshot {
            registration_revision: 0,
            ..original.clone()
        },
        RegistrationSnapshot {
            trust_revision: 0,
            ..original.clone()
        },
        RegistrationSnapshot {
            bindings: vec![original.bindings[0].clone(); 2],
            ..original.clone()
        },
        RegistrationSnapshot {
            approved_root_sha256: vec![vec![1; 31]],
            ..original.clone()
        },
        RegistrationSnapshot {
            approved_root_sha256: vec![vec![1; 32]; 2],
            ..original.clone()
        },
    ] {
        assert!(decode_registration(&message.encode_to_vec(), &config(), now).is_err());
    }
    let mut unknown_role = original.clone();
    unknown_role.bindings[0].role = EnumValue::from(999);
    assert!(decode_registration(&unknown_role.encode_to_vec(), &config(), now).is_err());
    let mut missing_application = original;
    missing_application.bindings[0].role = EnumValue::from(WireRole::Agent);
    assert!(decode_registration(&missing_application.encode_to_vec(), &config(), now).is_err());
    assert!(matches!(
        decode_registration(&[0xff], &config(), now),
        Err(RegistrationProjectionError::Malformed(_))
    ));
}

#[test]
fn projection_bytes_are_bounded_before_protobuf_decoding() {
    assert!(matches!(
        decode_registration(
            &vec![0; crate::constants::REGISTRATION_MAX_BYTES + 1],
            &config(),
            SystemTime::now()
        ),
        Err(RegistrationProjectionError::TooLarge)
    ));
}

#[test]
fn projection_bounds_sets_and_preserves_a_governed_supervisor_without_an_application() {
    let now = SystemTime::now();
    let original = snapshot(now);
    for message in [
        RegistrationSnapshot {
            bindings: vec![original.bindings[0].clone(); trogon_workload_identity::constants::MAX_BINDINGS + 1],
            ..original.clone()
        },
        RegistrationSnapshot {
            approved_root_sha256: vec![vec![1; 32]; trogon_workload_identity::constants::MAX_ROOTS + 1],
            ..original.clone()
        },
    ] {
        assert!(matches!(
            decode_registration(&message.encode_to_vec(), &config(), now),
            Err(RegistrationProjectionError::Registration(RegistrationError::Bounds))
        ));
    }
    let mut supervisor = original;
    supervisor.bindings[0].role = EnumValue::from(WireRole::Supervisor);
    let checked = decode_registration(&supervisor.encode_to_vec(), &config(), now).expect("governed supervisor");
    assert_eq!(checked.bindings()[0].role(), WorkloadRole::Supervisor);
    assert!(checked.bindings()[0].application_id().is_none());
}
