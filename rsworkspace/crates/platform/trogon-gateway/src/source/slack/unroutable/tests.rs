use super::*;
use crate::source::slack::header_text::DeliveryHeaders;

fn rendered(id: &UnroutableMessageId) -> String {
    let headers = DeliveryHeaders::new().with("Nats-Msg-Id", id.header()).into_inner();
    headers.get("Nats-Msg-Id").map(|v| v.as_str().to_owned()).unwrap()
}

#[test]
fn the_same_rejection_always_gets_the_same_message_id() {
    let first = UnroutableMessageId::new(UnroutableReason::InvalidJson, b"body");
    let second = UnroutableMessageId::new(UnroutableReason::InvalidJson, b"body");
    assert_eq!(rendered(&first), rendered(&second));
}

#[test]
fn different_bodies_or_reasons_get_different_message_ids() {
    let base = rendered(&UnroutableMessageId::new(UnroutableReason::InvalidJson, b"body"));
    let other_body = rendered(&UnroutableMessageId::new(UnroutableReason::InvalidJson, b"other"));
    let other_reason = rendered(&UnroutableMessageId::new(UnroutableReason::MissingEventId, b"body"));
    assert_ne!(base, other_body);
    assert_ne!(base, other_reason);
}

#[test]
fn message_id_is_a_prefixed_sha256_digest() {
    let id = rendered(&UnroutableMessageId::new(UnroutableReason::InvalidJson, b"body"));
    let digest = id.strip_prefix("unroutable-").unwrap();
    assert_eq!(digest.len(), 64);
    assert!(digest.bytes().all(|byte| byte.is_ascii_hexdigit()));
}

#[test]
fn every_reason_has_a_distinct_snake_case_name() {
    let reasons = [
        UnroutableReason::InvalidJson,
        UnroutableReason::UnhandledPayloadType,
        UnroutableReason::MissingEventType,
        UnroutableReason::MissingEventId,
        UnroutableReason::InvalidUtf8Form,
        UnroutableReason::UnrecognizedForm,
        UnroutableReason::InvalidInteractionJson,
        UnroutableReason::MissingInteractionType,
        UnroutableReason::MissingInteractionTriggerId,
        UnroutableReason::MissingCommand,
        UnroutableReason::MissingCommandTriggerId,
        UnroutableReason::InvalidHeaderValue,
        UnroutableReason::UnhandledSocketModeType,
        UnroutableReason::MissingEnvelopeId,
        UnroutableReason::MissingSocketModePayload,
    ];
    let names: std::collections::HashSet<_> = reasons.iter().map(|reason| reason.as_str()).collect();
    assert_eq!(names.len(), reasons.len());
    assert!(names.iter().all(|name| {
        name.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
    }));
}
