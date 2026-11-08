#![allow(clippy::expect_used)]
#![cfg(feature = "peers")]

use buffa::Message;
use serde_json::{Value, json};
use trogonai_proto::peers::v1alpha1::MessageMetadata;

fn roundtrip(payload: Value) -> Value {
    let metadata: MessageMetadata = serde_json::from_value(payload.clone()).expect("valid message metadata JSON");
    let wire = metadata.encode_to_vec();
    let decoded = MessageMetadata::decode_from_slice(&wire).expect("message metadata wire roundtrip");
    let retained = serde_json::to_value(decoded).expect("message metadata JSON roundtrip");
    assert_eq!(
        retained, payload,
        "persisted metadata must retain the supplied semantic fingerprint separately from ClaimCheck integrity"
    );
    retained
}

#[test]
fn metadata_roundtrip_keeps_semantic_identity_separate_from_claim_check_integrity() {
    let original = json!({
        "messageId": "message_alpha",
        "contentRef": {
            "storageRef": {"value": "sealed_first"},
            "digest": {"algorithm": "sha256", "value": "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA="}
        },
        "semanticFingerprint": {"value": "AQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQEBAQE="}
    });
    let mut resealed = original.clone();
    resealed["contentRef"]["storageRef"]["value"] = json!("sealed_second");
    resealed["contentRef"]["digest"]["value"] = json!("AgICAgICAgICAgICAgICAgICAgICAgICAgICAgICAgI=");
    let mut changed_semantics = original.clone();
    changed_semantics["semanticFingerprint"]["value"] = json!("AwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwMDAwM=");

    let original = roundtrip(original);
    let resealed = roundtrip(resealed);
    let changed_semantics = roundtrip(changed_semantics);
    assert_eq!(original["semanticFingerprint"], resealed["semanticFingerprint"]);
    assert_ne!(original["contentRef"], resealed["contentRef"]);
    assert_eq!(original["contentRef"], changed_semantics["contentRef"]);
    assert_ne!(
        original["semanticFingerprint"],
        changed_semantics["semanticFingerprint"]
    );
}
