use super::*;

/// The bridge parses updates the same way the pipeline does: bytes off the
/// wire, not a pre-built `serde_json::Value`. `teloxide`'s nested
/// `flatten`/`untagged` types round-trip through the streaming deserializer
/// but not through `Value`, so building the fixture this way is required, not
/// stylistic.
fn update_from(body: serde_json::Value) -> Update {
    let bytes = serde_json::to_vec(&body).expect("serialize update");
    serde_json::from_slice(&bytes).expect("deserialize update")
}

fn message_update(chat_id: i64, user_id: u64, text: &str) -> Update {
    update_from(serde_json::json!({
        "update_id": 1,
        "message": {
            "message_id": 1,
            "date": 1_700_000_000,
            "chat": { "id": chat_id, "type": "private", "first_name": "Test" },
            "from": { "id": user_id, "is_bot": false, "first_name": "Test" },
            "text": text,
        }
    }))
}

/// `inbound_event` only carries `UpdateKind::Message`. An edit is a real update
/// kind the raw stream keeps for later, and it must come back as `None` here
/// rather than being misread as a fresh message.
#[test]
fn an_edited_message_update_yields_no_inbound_event() {
    let update = update_from(serde_json::json!({
        "update_id": 1,
        "edited_message": {
            "message_id": 1,
            "date": 1_700_000_000,
            "chat": { "id": 42, "type": "private", "first_name": "Test" },
            "from": { "id": 42, "is_bot": false, "first_name": "Test" },
            "text": "edited",
        }
    }));

    let triggers = CommandTriggers::default();
    assert!(inbound_event(&update, "mybot", &triggers).is_none());
}

/// A chat id is always digits or a leading `-`, so `Endpoint::new` can never
/// reject the peer token built from one; the only way to reach this arm is a
/// misconfigured `bot_account`, which is what this pins.
#[test]
fn an_unsafe_bot_account_drops_the_update_instead_of_panicking() {
    let update = message_update(42, 42, "hello");
    let triggers = CommandTriggers::default();
    assert!(inbound_event(&update, "bad bot", &triggers).is_none());
}

/// An unsafe sender id can no longer reach this function: `PlatformUserId`
/// refuses to hold one, so the only token left that can spoil the endpoint is
/// the account the bridge was configured with.
#[test]
fn sender_endpoint_returns_none_for_an_unsafe_bot_account() {
    let sender = Sender {
        platform_user_id: PlatformUserId::new("42").expect("valid id"),
        display_name: "Test".to_string(),
    };
    assert!(sender_endpoint("bad bot", &sender).is_none());
    assert!(sender_endpoint("mybot", &sender).is_some());
}

/// The whole reason `sender_endpoint` exists: a group chat is one endpoint
/// shared by everyone in it, so its peer must be the sender's id and never the
/// chat's, or authorizing the chat would silently authorize the wrong party.
#[test]
fn sender_endpoint_peer_is_the_sender_not_the_chat() {
    let update = message_update(999, 42, "hello");
    let triggers = CommandTriggers::default();
    let event = inbound_event(&update, "mybot", &triggers).expect("event");

    let endpoint = sender_endpoint("mybot", &event.sender).expect("endpoint");
    assert_eq!(endpoint.peer(), "42");
    assert_ne!(endpoint.peer(), event.endpoint.peer());
}
