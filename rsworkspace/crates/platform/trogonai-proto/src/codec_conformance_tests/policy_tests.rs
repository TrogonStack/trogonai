use buffa::Message;
use serde_json::json;

use super::{assert_json_codec, assert_wire_codec};
use crate::policy::egress_permissions::v1alpha1::egress_destination::Destination;
use crate::policy::egress_permissions::v1alpha1::{EgressDestination, EgressHost, EgressPermissionPolicy};

fn policy_for(host: &str, port: u32) -> EgressPermissionPolicy {
    EgressPermissionPolicy {
        permitted: vec![EgressDestination {
            destination: Some(Destination::Host(Box::new(EgressHost {
                host: host.to_string(),
                port,
            }))),
        }],
    }
}

#[test]
fn egress_permission_policy_round_trips_through_json_and_wire() {
    let policy = assert_json_codec::<EgressPermissionPolicy>(json!({
        "permitted": [{"host": {"host": "api.example.com", "port": 443}}]
    }));

    assert_eq!(policy, policy_for("api.example.com", 443));
}

#[test]
fn egress_permission_policy_wire_layout_is_stable() {
    let wire = [
        0x0a, 0x0b, // permitted[0]
        0x0a, 0x09, // destination.host
        0x0a, 0x04, b'a', b'.', b'i', b'o', // host = "a.io"
        0x10, 0xbb, 0x03, // port = 443
    ];
    let policy = policy_for("a.io", 443);

    assert_eq!(policy.try_encode_to_vec().expect("encode"), wire);
    assert_wire_codec(&wire, &policy);
}

#[test]
fn truncated_egress_permission_policy_fails_closed() {
    let wire = [0x0a, 0x0a, 0x0a, 0x08, 0x0a, 0x04, b'a', b'.'];

    assert!(EgressPermissionPolicy::decode_from_slice(&wire).is_err());
}
