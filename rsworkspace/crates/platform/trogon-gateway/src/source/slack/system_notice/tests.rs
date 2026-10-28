use super::*;

#[test]
fn only_known_outer_types_are_system_notices() {
    assert_eq!(
        SystemNoticeType::from_payload_type("app_rate_limited"),
        Some(SystemNoticeType::AppRateLimited)
    );
    assert_eq!(SystemNoticeType::from_payload_type("event_callback"), None);
    assert_eq!(SystemNoticeType::from_payload_type(""), None);
}

#[test]
fn a_notice_type_renders_as_its_slack_name() {
    assert_eq!(SystemNoticeType::AppRateLimited.as_str(), "app_rate_limited");
}

#[test]
fn message_id_is_a_prefixed_sha256_of_the_body() {
    let id = SystemMessageId::new(b"body");
    let digest = id.as_str().strip_prefix("system-").unwrap();
    assert_eq!(digest.len(), 64);
    assert_eq!(
        digest,
        "230d8358dc8e8890b4c58deeb62912ee2f20357ae92a5cc861b98e68fe31acb5"
    );
}

#[test]
fn the_same_body_always_gets_the_same_message_id_and_other_bodies_differ() {
    assert_eq!(SystemMessageId::new(b"body"), SystemMessageId::new(b"body"));
    assert_ne!(SystemMessageId::new(b"body"), SystemMessageId::new(b"other"));
}
