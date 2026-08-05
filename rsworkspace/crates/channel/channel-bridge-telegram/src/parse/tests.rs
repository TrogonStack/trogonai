use super::*;

fn account() -> ChannelAccount {
    ChannelAccount::new("telegram", "mybot").expect("valid account")
}

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

/// A photo, which is where Telegram files the sender's words under `caption`
/// rather than `text`. The caption is omitted entirely when there is none, which
/// is a bare photo: media with nothing said about it.
fn photo_update(chat_id: i64, user_id: u64, caption: Option<&str>) -> Update {
    let mut message = serde_json::json!({
        "message_id": 1,
        "date": 1_700_000_000,
        "chat": { "id": chat_id, "type": "private", "first_name": "Test" },
        "from": { "id": user_id, "is_bot": false, "first_name": "Test" },
        "photo": [{
            "file_id": "photo-file-id",
            "file_unique_id": "photo-unique-id",
            "width": 320,
            "height": 320,
            "file_size": 3452,
        }],
    });
    if let Some(caption) = caption {
        message["caption"] = serde_json::json!(caption);
    }
    update_from(serde_json::json!({ "update_id": 1, "message": message }))
}

/// A caption is the user talking, so it reaches the agent as the message text and
/// is read for a trigger like any other sentence. Nothing else about the photo is
/// carried: redeeming the handle is out of band (ADR#0044), and waiting for a
/// downloader that does not exist yet would lose the words too.
#[test]
fn a_captioned_photo_carries_the_caption_as_the_message_text() {
    let triggers = CommandTriggers::default();

    let update = photo_update(42, 42, Some("what does this say?"));
    let event = inbound_event(&update, &account(), &triggers).expect("event");
    assert_eq!(event.text.as_deref(), Some("what does this say?"));
    assert!(event.command.is_none());
    assert!(event.attachments.is_empty(), "no handle may be named before ADR#0044");

    let update = photo_update(42, 42, Some("/new read this one"));
    let event = inbound_event(&update, &account(), &triggers).expect("event");
    assert_eq!(event.command, Some(trogon_channel::Command::NewSession));
    assert_eq!(event.text.as_deref(), Some("read this one"));
}

/// A photo with nothing said about it is still dropped: there are no words to
/// forward, and the bytes are the downloader's to fetch, so an event here would
/// prompt the agent with an empty turn.
#[test]
fn a_photo_with_no_caption_yields_no_inbound_event() {
    let update = photo_update(42, 42, None);
    let triggers = CommandTriggers::default();
    assert!(inbound_event(&update, &account(), &triggers).is_none());
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
    assert!(inbound_event(&update, &account(), &triggers).is_none());
}

/// A group chat is numbered negatively, and that minus sign has to survive into
/// the endpoint: a peer token is what the principal lookup is keyed by, so a
/// mangled one authorizes nobody.
#[test]
fn a_group_chats_negative_id_reaches_the_endpoint_intact() {
    let update = message_update(-1_001_234_567_890, 42, "hello");
    let triggers = CommandTriggers::default();
    let event = inbound_event(&update, &account(), &triggers).expect("event");

    assert_eq!(event.endpoint.peer(), "-1001234567890");
    assert_eq!(event.endpoint.kv_key(), "telegram.mybot.-1001234567890");
}

/// The whole reason `sender_endpoint` exists: a group chat is one endpoint
/// shared by everyone in it, so its peer must be the sender's id and never the
/// chat's, or authorizing the chat would silently authorize the wrong party.
#[test]
fn sender_endpoint_peer_is_the_sender_not_the_chat() {
    let update = message_update(999, 42, "hello");
    let triggers = CommandTriggers::default();
    let event = inbound_event(&update, &account(), &triggers).expect("event");

    let endpoint = sender_endpoint(&account(), &event.sender);
    assert_eq!(endpoint.peer(), "42");
    assert_ne!(endpoint.peer(), event.endpoint.peer());
}
