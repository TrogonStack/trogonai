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
