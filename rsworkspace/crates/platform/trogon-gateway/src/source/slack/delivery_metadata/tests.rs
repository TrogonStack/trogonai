use super::*;

fn http_context(pairs: &[(&str, &str)]) -> DeliveryContext {
    let mut headers = HeaderMap::new();
    for (name, value) in pairs {
        headers.append(
            axum::http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
            axum::http::HeaderValue::from_str(value).unwrap(),
        );
    }
    DeliveryContext::http(&headers)
}

fn applied(context: &DeliveryContext) -> async_nats::HeaderMap {
    context.apply(DeliveryHeaders::new()).into_inner()
}

#[test]
fn http_without_retry_headers_carries_nothing() {
    let headers = applied(&http_context(&[]));

    assert!(headers.get(NATS_HEADER_RETRY_NUM).is_none());
    assert!(headers.get(NATS_HEADER_RETRY_REASON).is_none());
}

#[test]
fn http_carries_retry_number_and_reason() {
    let headers = applied(&http_context(&[
        (HEADER_RETRY_NUM, "2"),
        (HEADER_RETRY_REASON, "http_timeout"),
    ]));

    assert_eq!(headers.get(NATS_HEADER_RETRY_NUM).map(|v| v.as_str()), Some("2"));
    assert_eq!(
        headers.get(NATS_HEADER_RETRY_REASON).map(|v| v.as_str()),
        Some("http_timeout")
    );
}

#[test]
fn http_uses_the_first_value_of_a_repeated_header() {
    let headers = applied(&http_context(&[(HEADER_RETRY_NUM, "1"), (HEADER_RETRY_NUM, "2")]));

    assert_eq!(headers.get(NATS_HEADER_RETRY_NUM).map(|v| v.as_str()), Some("1"));
}

#[test]
fn http_ignores_a_retry_number_that_is_not_an_integer() {
    let headers = applied(&http_context(&[(HEADER_RETRY_NUM, "soon")]));

    assert!(headers.get(NATS_HEADER_RETRY_NUM).is_none());
}

#[test]
fn http_ignores_header_values_that_are_not_text() {
    let mut headers = HeaderMap::new();
    headers.insert(
        HEADER_RETRY_REASON,
        axum::http::HeaderValue::from_bytes(&[0xff, 0xfe]).unwrap(),
    );

    assert_eq!(DeliveryContext::http(&headers), DeliveryContext::default());
}

#[test]
fn leading_integer_follows_parse_int() {
    assert_eq!(parse_leading_integer("3"), Some(3));
    assert_eq!(parse_leading_integer("  7x"), Some(7));
    assert_eq!(parse_leading_integer("+4"), Some(4));
    assert_eq!(parse_leading_integer("-1"), Some(-1));
    assert_eq!(parse_leading_integer("x1"), None);
    assert_eq!(parse_leading_integer(""), None);
    assert_eq!(parse_leading_integer("99999999999999999999"), None);
}

#[test]
fn socket_carries_retry_attempt_and_reason() {
    let headers = applied(&DeliveryContext::socket(Some(1), Some("timeout")).unwrap());

    assert_eq!(headers.get(NATS_HEADER_RETRY_NUM).map(|v| v.as_str()), Some("1"));
    assert_eq!(
        headers.get(NATS_HEADER_RETRY_REASON).map(|v| v.as_str()),
        Some("timeout")
    );
}

#[test]
fn socket_treats_a_zero_attempt_and_empty_reason_as_a_first_delivery() {
    let context = DeliveryContext::socket(Some(0), Some("")).unwrap();
    let headers = applied(&context);

    assert_eq!(context, DeliveryContext::default());
    assert!(headers.get(NATS_HEADER_RETRY_NUM).is_none());
    assert!(headers.get(NATS_HEADER_RETRY_REASON).is_none());
}

#[test]
fn socket_without_retry_fields_carries_nothing() {
    assert_eq!(DeliveryContext::socket(None, None).unwrap(), DeliveryContext::default());
}

#[test]
fn socket_rejects_an_unsafe_retry_reason() {
    assert_eq!(
        DeliveryContext::socket(None, Some("a\r\nb")).unwrap_err(),
        InvalidHeaderTextError
    );
}

#[test]
fn delivery_metadata_applies_context_and_payload_together() {
    let context = DeliveryContext::socket(Some(1), None).unwrap();
    let payload = serde_json::json!({ "api_app_id": "A1234" });

    let headers = DeliveryMetadata::new(&context, PayloadMetadata::event(&payload))
        .apply(DeliveryHeaders::new())
        .unwrap()
        .into_inner();

    assert_eq!(headers.get(NATS_HEADER_RETRY_NUM).map(|v| v.as_str()), Some("1"));
    assert_eq!(
        headers
            .get(super::super::constants::NATS_HEADER_API_APP_ID)
            .map(|v| v.as_str()),
        Some("A1234")
    );
}
