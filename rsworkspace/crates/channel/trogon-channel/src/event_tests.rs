use super::*;

#[test]
fn a_platform_user_id_must_be_an_endpoint_token() {
    assert_eq!(PlatformUserId::new("42").expect("valid").as_str(), "42");
    assert_eq!(
        PlatformUserId::new("user id").unwrap_err(),
        EndpointError::InvalidCharacter(' ')
    );
    // A `.` would split the composite endpoint key it becomes part of.
    assert_eq!(
        PlatformUserId::new("4.2").unwrap_err(),
        EndpointError::InvalidCharacter('.')
    );
    assert_eq!(PlatformUserId::new("").unwrap_err(), EndpointError::Empty);
}

/// The point of the type: channel-provided JSON cannot produce an id the
/// constructor would have rejected, which is what makes deriving `Deserialize`
/// on `InboundEvent` safe without a separate wire twin.
#[test]
fn deserializing_a_sender_rejects_an_id_the_constructor_would_reject() {
    let ok: Sender = serde_json::from_str(r#"{"platform_user_id":"42","display_name":"Ada"}"#).expect("valid sender");
    assert_eq!(ok.platform_user_id.as_str(), "42");

    let err = serde_json::from_str::<Sender>(r#"{"platform_user_id":"user id","display_name":"Ada"}"#)
        .expect_err("unsafe id must not deserialize");
    assert_eq!(err.classify(), serde_json::error::Category::Data, "{err}");
}

/// A display name is shown, never matched on, so it holds no invariant: names
/// really do contain spaces, dots, and emoji.
#[test]
fn a_display_name_is_free_form() {
    let sender: Sender =
        serde_json::from_str(r#"{"platform_user_id":"42","display_name":"Ada L. 👋"}"#).expect("valid sender");
    assert_eq!(sender.display_name, "Ada L. 👋");
}

#[test]
fn a_message_ref_only_rejects_blankness() {
    assert_eq!(MessageRef::new("1").expect("valid").as_str(), "1");
    assert_eq!(MessageRef::new("  ").unwrap_err(), EventFieldError::BlankMessageRef);
    assert_eq!(MessageRef::new("").unwrap_err(), EventFieldError::BlankMessageRef);
}

/// Looser than an endpoint token on purpose: this type is channel-neutral, and
/// message ids on other channels are not tokens.
#[test]
fn a_message_ref_accepts_an_email_style_id() {
    let reference = MessageRef::new("<CAF=abc.123@mail.example.com>").expect("valid");
    assert_eq!(reference.as_str(), "<CAF=abc.123@mail.example.com>");
}

#[test]
fn a_media_type_normalizes_case_so_comparisons_hold() {
    assert_eq!(MimeType::new("IMAGE/PNG").expect("valid").as_str(), "image/png");
    assert_eq!(MimeType::new("  image/png  ").expect("valid").as_str(), "image/png");
    assert_eq!(
        MimeType::new("image/png").expect("valid"),
        MimeType::new("Image/PNG").expect("valid")
    );
}

#[test]
fn a_media_type_needs_a_type_and_a_subtype() {
    for (raw, reason) in [
        ("image", MediaTypeError::MissingSeparator),
        ("", MediaTypeError::MissingSeparator),
        ("image/", MediaTypeError::EmptySubtype),
        ("image/;charset=utf-8", MediaTypeError::EmptySubtype),
        ("/png", MediaTypeError::EmptyType),
        ("image/png/extra", MediaTypeError::SubtypeIsNotOne),
        ("image / png", MediaTypeError::InteriorWhitespace),
    ] {
        assert_eq!(
            MimeType::new(raw).unwrap_err(),
            EventFieldError::NotAMediaType(reason),
            "{raw:?} must not be a media type"
        );
    }
}

#[test]
fn a_media_type_keeps_parameters() {
    assert_eq!(
        MimeType::new("text/plain;charset=utf-8").expect("valid").as_str(),
        "text/plain;charset=utf-8"
    );
}

/// Case-insensitivity is defined for the type and the subtype only. A
/// `multipart` boundary is a delimiter the sender picked and has to survive as
/// typed, or the body it delimits stops being parseable.
#[test]
fn a_media_type_normalizes_the_subtype_without_touching_its_parameters() {
    assert_eq!(
        MimeType::new("MULTIPART/Mixed;boundary=AbCd").expect("valid").as_str(),
        "multipart/mixed;boundary=AbCd"
    );
}

/// The handle doubles as the readiness key in `channel_media_{prefix}`
/// (ADR#0044), so it has to be safe as a KV key.
#[test]
fn a_platform_ref_must_be_usable_as_a_kv_key() {
    let handle = PlatformRef::new("AgACAgQAAx0-Ef_9").expect("valid");
    assert_eq!(handle.kv_key(), "AgACAgQAAx0-Ef_9");
    assert!(PlatformRef::new("has space").is_err());
    assert!(PlatformRef::new("has.dot").is_err());
    assert!(PlatformRef::new("").is_err());
}

/// Why `parse` carries no error arm for a numeric id: a platform that numbers
/// its users and messages can only ever produce a token, so the checked path and
/// the unchecked one have to agree for every integer either could see.
#[test]
fn a_numeric_platform_id_needs_no_validation() {
    assert_eq!(PlatformUserId::from(u64::MAX).as_str(), u64::MAX.to_string());
    assert_eq!(
        PlatformUserId::from(42),
        PlatformUserId::new("42").expect("the checked path agrees")
    );

    // A message id may be negative, and `-` is an allowed character.
    assert_eq!(MessageRef::from(i64::MIN).as_str(), i64::MIN.to_string());
    assert_eq!(
        MessageRef::from(-7),
        MessageRef::new("-7").expect("the checked path agrees")
    );
}

/// Each of these is printed next to the key or id it came from: the pipeline
/// logs `sender = %event.sender.platform_user_id`, and `parse` logs the handle
/// it could not encode. A `Display` that diverged from `as_str` would leave an
/// operator grepping the KV store for a value that was never printed.
#[test]
fn a_value_object_displays_as_the_scalar_it_wraps() {
    assert_eq!(PlatformUserId::new("42").expect("id").to_string(), "42");
    assert_eq!(MessageRef::new("7").expect("ref").to_string(), "7");
    assert_eq!(MimeType::new("IMAGE/PNG").expect("mime").to_string(), "image/png");

    let handle = PlatformRef::new("file-abc").expect("handle");
    assert_eq!(handle.as_str(), "file-abc");
    assert_eq!(handle.to_string(), handle.kv_key());
}

#[test]
fn an_attachment_kind_is_a_closed_set() {
    let kind: AttachmentKind = serde_json::from_str(r#""voice""#).expect("known kind");
    assert_eq!(kind, AttachmentKind::Voice);
    assert_eq!(
        serde_json::to_string(&AttachmentKind::Document).expect("json"),
        r#""document""#
    );
    assert!(serde_json::from_str::<AttachmentKind>(r#""hologram""#).is_err());
}

/// The value objects serialize as the bare scalars they wrap, so the `_meta`
/// shape the architecture doc documents is unchanged by introducing them.
#[test]
fn an_event_serializes_its_value_objects_transparently() {
    let event = InboundEvent {
        endpoint: Endpoint::new("telegram", "mybot", "42").expect("endpoint"),
        sender: Sender {
            platform_user_id: PlatformUserId::new("42").expect("id"),
            display_name: "Ada".to_string(),
        },
        text: Some("hello".to_string()),
        command: None,
        attachments: vec![Attachment {
            kind: AttachmentKind::Image,
            mime: MimeType::new("image/png").expect("mime"),
            size: 1024,
            platform_ref: PlatformRef::new("file-abc").expect("handle"),
        }],
        message_ref: MessageRef::new("7").expect("ref"),
        occurred_at: 1_700_000_000,
    };

    let json = serde_json::to_value(&event).expect("serialize");
    assert_eq!(json["sender"]["platform_user_id"], "42");
    assert_eq!(json["message_ref"], "7");
    assert_eq!(json["attachments"][0]["kind"], "image");
    assert_eq!(json["attachments"][0]["mime"], "image/png");
    assert_eq!(json["attachments"][0]["platform_ref"], "file-abc");

    let round_tripped: InboundEvent = serde_json::from_value(json).expect("deserialize");
    assert_eq!(round_tripped.message_ref, event.message_ref);
    assert_eq!(round_tripped.attachments[0].mime, event.attachments[0].mime);
}
