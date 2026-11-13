#![cfg(feature = "peers")]

use buffa::{HasMessageView, Message, MessageView, ViewEncode};
use trogonai_proto::peers::v1alpha1::RecipientReceipt;

#[test]
fn receipt_sender_binding_uses_field_seven_and_preserves_legacy_absence() -> Result<(), Box<dyn std::error::Error>> {
    let legacy = RecipientReceipt::decode_from_slice(b"\x0a\x09message_0")?;
    assert_eq!(legacy.message_id.as_deref(), Some("message_0"));
    assert!(legacy.sender_peer_id.is_none());
    assert_eq!(legacy.encode_to_vec(), b"\x0a\x09message_0");
    let bound = RecipientReceipt {
        sender_peer_id: Some("peer_alpha".into()),
        ..Default::default()
    };
    let wire = bound.encode_to_vec();
    assert_eq!(wire, b"\x3a\x0apeer_alpha");
    assert_eq!(RecipientReceipt::decode_from_slice(&wire)?, bound);
    let view = RecipientReceipt::decode_view(&wire)?;
    assert_eq!(view.to_owned_message()?, bound);
    assert_eq!(view.try_encode_to_vec()?, wire);
    let retained = RecipientReceipt::decode_view_handle(buffa::bytes::Bytes::from(wire.clone()))?;
    assert_eq!(retained.sender_peer_id(), Some("peer_alpha"));
    assert_eq!(retained.to_owned_message(), bound);
    let json = serde_json::to_value(&bound)?;
    assert_eq!(json, serde_json::json!({"senderPeerId": "peer_alpha"}));
    assert_eq!(serde_json::from_value::<RecipientReceipt>(json)?, bound);
    let absent = RecipientReceipt::decode_view_handle(buffa::bytes::Bytes::new())?;
    assert!(absent.sender_peer_id().is_none());
    Ok(())
}
