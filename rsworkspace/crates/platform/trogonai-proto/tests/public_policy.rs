#![cfg(feature = "policy")]
#![allow(clippy::expect_used)]

use buffa::Message;
use trogonai_proto::connections::types::v1alpha1::ConnectionId;
use trogonai_proto::policy::egress_permissions::v1alpha1::EgressPermissionPolicy;

#[test]
fn policy_feature_preserves_public_connection_identity_wire_contract() {
    let connection = ConnectionId {
        value: "c1".to_string(),
    };
    let wire = connection.try_encode_to_vec().expect("encode connection identity");

    assert_eq!(wire, [0x0a, 0x02, b'c', b'1']);
    assert_eq!(
        ConnectionId::decode_from_slice(&wire).expect("decode connection identity"),
        connection
    );
}

#[test]
fn policy_feature_preserves_public_egress_policy_wire_contract() {
    let json = serde_json::json!({
        "permitted": [{"host": {"host": "a.io", "port": 443}}]
    });
    let policy: EgressPermissionPolicy = serde_json::from_value(json.clone()).expect("decode policy JSON");
    let wire = policy.try_encode_to_vec().expect("encode policy");

    assert_eq!(
        wire,
        [
            0x0a, 0x0b, 0x0a, 0x09, 0x0a, 0x04, b'a', b'.', b'i', b'o', 0x10, 0xbb, 0x03
        ]
    );
    assert_eq!(
        EgressPermissionPolicy::decode_from_slice(&wire).expect("decode policy wire"),
        policy
    );
    assert_eq!(serde_json::to_value(policy).expect("encode policy JSON"), json);
}
