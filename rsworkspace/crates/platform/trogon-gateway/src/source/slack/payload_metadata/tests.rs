use serde_json::json;

use super::*;

fn headers_of(metadata: PayloadMetadata<'_>) -> async_nats::HeaderMap {
    metadata.apply(DeliveryHeaders::new()).unwrap().into_inner()
}

fn header(headers: &async_nats::HeaderMap, name: &str) -> Option<String> {
    headers.get(name).map(|value| value.as_str().to_owned())
}

fn identity(payload: &Value) -> (Option<String>, Option<String>, Option<String>) {
    let headers = headers_of(PayloadMetadata::event(payload));
    (
        header(&headers, NATS_HEADER_ENTERPRISE_ID),
        header(&headers, NATS_HEADER_IS_ENTERPRISE_INSTALL),
        header(&headers, NATS_HEADER_API_APP_ID),
    )
}

fn some(value: &str) -> Option<String> {
    Some(value.to_owned())
}

#[test]
fn an_empty_payload_is_a_single_workspace_install_without_ids() {
    assert_eq!(identity(&json!({})), (None, some("false"), None));
}

#[test]
fn event_takes_identity_from_the_first_authorization() {
    let payload = json!({
        "enterprise_id": "E_TOP",
        "is_enterprise_install": false,
        "api_app_id": "A1234",
        "authorizations": [
            { "enterprise_id": "E1234", "is_enterprise_install": true },
            { "enterprise_id": "E_OTHER", "is_enterprise_install": false }
        ]
    });

    assert_eq!(identity(&payload), (some("E1234"), some("true"), some("A1234")));
}

#[test]
fn event_authorization_without_an_enterprise_has_none() {
    let payload = json!({
        "enterprise_id": "E_TOP",
        "authorizations": [{ "enterprise_id": null, "is_enterprise_install": false }]
    });

    assert_eq!(identity(&payload), (None, some("false"), None));
}

#[test]
fn event_without_authorizations_falls_back_to_the_top_level_fields() {
    let payload = json!({ "enterprise_id": "E_TOP", "is_enterprise_install": true, "authorizations": [] });

    assert_eq!(identity(&payload), (some("E_TOP"), some("true"), None));
}

#[test]
fn interaction_prefers_the_enterprise_object() {
    let payload = json!({
        "enterprise": { "id": "E1234" },
        "team": { "id": "T1234", "enterprise_id": "E_TEAM" },
        "is_enterprise_install": true,
        "api_app_id": "A1234"
    });

    let headers = headers_of(PayloadMetadata::interaction(&payload));

    assert_eq!(header(&headers, NATS_HEADER_ENTERPRISE_ID), some("E1234"));
    assert_eq!(header(&headers, NATS_HEADER_IS_ENTERPRISE_INSTALL), some("true"));
    assert_eq!(header(&headers, NATS_HEADER_API_APP_ID), some("A1234"));
}

#[test]
fn interaction_falls_back_to_the_team_enterprise_id() {
    let payload = json!({ "enterprise": null, "team": { "id": "T1234", "enterprise_id": "E_TEAM" } });

    let headers = headers_of(PayloadMetadata::interaction(&payload));

    assert_eq!(header(&headers, NATS_HEADER_ENTERPRISE_ID), some("E_TEAM"));
    assert_eq!(header(&headers, NATS_HEADER_IS_ENTERPRISE_INSTALL), some("false"));
}

#[test]
fn interaction_without_enterprise_or_team_has_no_enterprise() {
    let headers = headers_of(PayloadMetadata::interaction(&json!({ "team": null })));

    assert_eq!(header(&headers, NATS_HEADER_ENTERPRISE_ID), None);
}

#[test]
fn command_json_reads_string_and_boolean_install_flags() {
    for (flag, expected) in [
        (json!("true"), "true"),
        (json!("false"), "false"),
        (json!(true), "true"),
        (json!(7), "false"),
    ] {
        let payload = json!({ "enterprise_id": "E1234", "api_app_id": "A1234", "is_enterprise_install": flag });

        let headers = headers_of(PayloadMetadata::command_value(&payload));

        assert_eq!(header(&headers, NATS_HEADER_IS_ENTERPRISE_INSTALL), some(expected));
        assert_eq!(header(&headers, NATS_HEADER_ENTERPRISE_ID), some("E1234"));
        assert_eq!(header(&headers, NATS_HEADER_API_APP_ID), some("A1234"));
    }
}

#[test]
fn command_fields_read_the_form_values() {
    let fields = vec![
        ("enterprise_id".to_owned(), "E1234".to_owned()),
        ("is_enterprise_install".to_owned(), "true".to_owned()),
        ("api_app_id".to_owned(), "A1234".to_owned()),
    ];

    let headers = headers_of(PayloadMetadata::command_fields(&fields));

    assert_eq!(header(&headers, NATS_HEADER_ENTERPRISE_ID), some("E1234"));
    assert_eq!(header(&headers, NATS_HEADER_IS_ENTERPRISE_INSTALL), some("true"));
    assert_eq!(header(&headers, NATS_HEADER_API_APP_ID), some("A1234"));
}

#[test]
fn command_fields_without_values_are_a_single_workspace_install() {
    let headers = headers_of(PayloadMetadata::command_fields(&[]));

    assert_eq!(header(&headers, NATS_HEADER_ENTERPRISE_ID), None);
    assert_eq!(header(&headers, NATS_HEADER_IS_ENTERPRISE_INSTALL), some("false"));
    assert_eq!(header(&headers, NATS_HEADER_API_APP_ID), None);
}

#[test]
fn empty_ids_are_not_published() {
    let payload = json!({ "enterprise_id": "", "api_app_id": "" });
    let fields = vec![
        ("enterprise_id".to_owned(), String::new()),
        ("api_app_id".to_owned(), String::new()),
    ];

    for headers in [
        headers_of(PayloadMetadata::event(&payload)),
        headers_of(PayloadMetadata::command_value(&payload)),
        headers_of(PayloadMetadata::command_fields(&fields)),
    ] {
        assert_eq!(header(&headers, NATS_HEADER_ENTERPRISE_ID), None);
        assert_eq!(header(&headers, NATS_HEADER_API_APP_ID), None);
    }
}

#[test]
fn unsafe_ids_are_rejected() {
    let payload = json!({ "api_app_id": "A1\r\nX: 1" });

    let result = PayloadMetadata::event(&payload).apply(DeliveryHeaders::new());

    assert_eq!(result.unwrap_err(), InvalidHeaderTextError);
}

#[test]
fn event_carries_its_subtype_and_channel_type() {
    let payload = json!({ "event": { "type": "message", "subtype": "bot_message", "channel_type": "im" } });

    let headers = headers_of(PayloadMetadata::event(&payload));

    assert_eq!(header(&headers, NATS_HEADER_EVENT_SUBTYPE), some("bot_message"));
    assert_eq!(header(&headers, NATS_HEADER_CHANNEL_TYPE), some("im"));
}

#[test]
fn event_without_subtype_or_channel_type_carries_neither() {
    let headers = headers_of(PayloadMetadata::event(
        &json!({ "event": { "type": "app_home_opened" } }),
    ));

    assert_eq!(header(&headers, NATS_HEADER_EVENT_SUBTYPE), None);
    assert_eq!(header(&headers, NATS_HEADER_CHANNEL_TYPE), None);
}

#[test]
fn only_events_carry_event_metadata() {
    let payload =
        json!({ "type": "block_actions", "event": { "subtype": "x", "channel_type": "im" }, "command": "/x" });

    for metadata in [
        PayloadMetadata::interaction(&payload),
        PayloadMetadata::command_value(&payload),
        PayloadMetadata::command_fields(&[]),
    ] {
        let headers = headers_of(metadata);
        assert_eq!(header(&headers, NATS_HEADER_EVENT_SUBTYPE), None);
        assert_eq!(header(&headers, NATS_HEADER_CHANNEL_TYPE), None);
    }
}

#[test]
fn unsafe_event_metadata_is_rejected() {
    for event in [json!({ "subtype": "a\nb" }), json!({ "channel_type": "a\nb" })] {
        let payload = json!({ "event": event });

        let result = PayloadMetadata::event(&payload).apply(DeliveryHeaders::new());

        assert_eq!(result.unwrap_err(), InvalidHeaderTextError);
    }
}
