use super::*;

fn from_status(status: StatusCode, body: &str) -> DaytonaApiError {
    DaytonaApiError::from_status(status, Some(7), body.as_bytes())
}

#[test]
fn each_status_maps_to_its_variant() {
    let body = r#"{"statusCode":1,"message":"nope"}"#;
    let message = || "nope".to_owned();
    assert_eq!(
        from_status(StatusCode::UNAUTHORIZED, body),
        DaytonaApiError::Unauthorized(message())
    );
    assert_eq!(
        from_status(StatusCode::FORBIDDEN, body),
        DaytonaApiError::Unauthorized(message())
    );
    assert_eq!(
        from_status(StatusCode::NOT_FOUND, body),
        DaytonaApiError::NotFound(message())
    );
    assert_eq!(
        from_status(StatusCode::CONFLICT, body),
        DaytonaApiError::Conflict(message())
    );
    assert_eq!(
        from_status(StatusCode::TOO_MANY_REQUESTS, body),
        DaytonaApiError::RateLimited {
            retry_after_secs: Some(7),
            message: message()
        }
    );
    assert_eq!(
        from_status(StatusCode::BAD_GATEWAY, body),
        DaytonaApiError::Unavailable {
            status: 502,
            message: message()
        }
    );
    assert_eq!(
        from_status(StatusCode::BAD_REQUEST, body),
        DaytonaApiError::Rejected {
            status: 400,
            message: message()
        }
    );
}

#[test]
fn the_message_falls_back_to_the_status() {
    for body in ["", "not json", r#"{"message":5}"#, "{}"] {
        assert_eq!(
            from_status(StatusCode::NOT_FOUND, body),
            DaytonaApiError::NotFound("Not Found".to_owned()),
            "{body:?}"
        );
    }
    let unknown = StatusCode::from_u16(499).ok();
    assert!(unknown.is_some_and(|status| matches!(
        from_status(status, ""),
        DaytonaApiError::Rejected { status: 499, ref message } if message == "error"
    )));
}

#[test]
fn a_long_message_is_cut_to_the_limit() {
    let body = format!(r#"{{"message":"{}"}}"#, "m".repeat(MESSAGE_LIMIT + 50));
    assert!(matches!(
        from_status(StatusCode::CONFLICT, &body),
        DaytonaApiError::Conflict(ref message) if message.chars().count() == MESSAGE_LIMIT
    ));
}

#[test]
fn a_malformed_response_body_is_a_transport_error() {
    let parse = serde_json::from_str::<u8>("x");
    assert!(matches!(
        parse.map_err(DaytonaApiError::unexpected),
        Err(DaytonaApiError::Transport(_))
    ));
}

#[test]
fn the_rate_limit_message_carries_the_retry_hint_when_known() {
    assert_eq!(retry_hint(None), "");
    assert_eq!(retry_hint(Some(3)), ", retry after 3s");
}
