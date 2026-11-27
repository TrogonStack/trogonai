use super::*;

fn from_status(status: StatusCode, body: &str) -> VercelApiError {
    VercelApiError::from_status(status, Some(7), body.as_bytes())
}

#[test]
fn each_status_maps_to_its_variant() {
    let body = r#"{"error":{"code":"x","message":"nope"}}"#;
    let message = || "nope".to_owned();
    assert_eq!(
        from_status(StatusCode::UNAUTHORIZED, body),
        VercelApiError::Unauthorized(message())
    );
    assert_eq!(
        from_status(StatusCode::FORBIDDEN, body),
        VercelApiError::Unauthorized(message())
    );
    assert_eq!(
        from_status(StatusCode::NOT_FOUND, body),
        VercelApiError::NotFound(message())
    );
    assert_eq!(
        from_status(StatusCode::CONFLICT, body),
        VercelApiError::Conflict(message())
    );
    assert_eq!(
        from_status(StatusCode::TOO_MANY_REQUESTS, body),
        VercelApiError::RateLimited {
            retry_after_secs: Some(7),
            message: message()
        }
    );
    assert_eq!(
        from_status(StatusCode::BAD_GATEWAY, body),
        VercelApiError::Unavailable {
            status: 502,
            message: message()
        }
    );
    assert_eq!(
        from_status(StatusCode::BAD_REQUEST, body),
        VercelApiError::Rejected {
            status: 400,
            message: message()
        }
    );
}

#[test]
fn the_message_comes_from_the_body_or_falls_back_to_the_status() {
    assert_eq!(
        from_status(StatusCode::NOT_FOUND, r#"{"message":"top level"}"#),
        VercelApiError::NotFound("top level".to_owned())
    );
    for body in ["", "not json", r#"{"error":{}}"#, r#"{"error":{"message":5}}"#] {
        assert_eq!(
            from_status(StatusCode::NOT_FOUND, body),
            VercelApiError::NotFound("Not Found".to_owned()),
            "{body:?}"
        );
    }
    let unknown = StatusCode::from_u16(499).ok();
    assert!(unknown.is_some_and(|status| matches!(
        from_status(status, ""),
        VercelApiError::Rejected { status: 499, ref message } if message == "error"
    )));
}

#[test]
fn a_long_message_is_cut_to_the_limit() {
    let body = format!(r#"{{"message":"{}"}}"#, "m".repeat(MESSAGE_LIMIT + 50));
    assert!(matches!(
        from_status(StatusCode::CONFLICT, &body),
        VercelApiError::Conflict(ref message) if message.chars().count() == MESSAGE_LIMIT
    ));
}

#[test]
fn a_malformed_response_body_is_a_transport_error() {
    let parse = serde_json::from_str::<u8>("x");
    assert!(matches!(
        parse.map_err(VercelApiError::unexpected),
        Err(VercelApiError::Transport(_))
    ));
}

#[test]
fn the_rate_limit_message_carries_the_retry_hint_when_known() {
    assert_eq!(retry_hint(None), "");
    assert_eq!(retry_hint(Some(3)), ", retry after 3s");
}
