use super::*;
use std::collections::HashMap;

#[test]
fn pulled_message_carries_every_field_of_the_pubsub_message() {
    let message: Message = serde_json::from_value(serde_json::json!({
        "data": "cGF5bG9hZA==",
        "messageId": "1234",
        "orderingKey": "order-1",
        "attributes": { "k": "v" },
        "publishTime": "2023-11-14T22:13:20Z",
    }))
    .unwrap();

    let pulled = PulledMessage::from(message);

    assert_eq!(pulled.data, bytes::Bytes::from_static(b"payload"));
    assert_eq!(pulled.message_id, "1234");
    assert_eq!(pulled.ordering_key, "order-1");
    assert_eq!(
        pulled.attributes,
        HashMap::from([("k".to_string(), "v".to_string())])
            .into_iter()
            .collect()
    );
    assert_eq!(pulled.publish_time.as_deref(), Some("2023-11-14T22:13:20Z"));
}

#[test]
fn pulled_message_without_publish_time_has_none() {
    let pulled = PulledMessage::from(Message::new().set_message_id("1"));

    assert_eq!(pulled.publish_time, None);
    assert!(pulled.attributes.is_empty());
    assert!(pulled.data.is_empty());
}
