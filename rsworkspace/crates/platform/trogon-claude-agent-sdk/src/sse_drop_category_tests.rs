use super::*;

#[test]
fn names_are_the_transport_categories() {
    let names = [
        (SseDropCategory::CatchUpTruncated, "catch_up_truncated"),
        (SseDropCategory::MalformedDeliveryUpdate, "malformed_delivery_update"),
        (SseDropCategory::MissingType, "missing_type"),
        (SseDropCategory::EphemeralType, "ephemeral_type"),
        (SseDropCategory::MalformedControlResponse, "malformed_control_response"),
        (SseDropCategory::UserWorkerOnly, "user_worker_only"),
        (SseDropCategory::MalformedUserContent, "malformed_user_content"),
        (SseDropCategory::ForgedReply, "forged_reply"),
        (SseDropCategory::ExpectedEcho, "expected_echo"),
        (SseDropCategory::NonWorkerType, "non_worker_type"),
        (SseDropCategory::UnparseableFrame, "unparseable_frame"),
    ];
    for (category, name) in names {
        assert_eq!(category.as_str(), name);
    }
}
