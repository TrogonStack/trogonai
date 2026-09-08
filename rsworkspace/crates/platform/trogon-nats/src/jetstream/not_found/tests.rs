use super::*;

/// A server-side JetStream API error, built the way the wire delivers it so the
/// `err_code` under test is the one a real server would send.
fn api_error(code: u16, err_code: u64, description: &str) -> async_nats::jetstream::Error {
    serde_json::from_value(serde_json::json!({
        "code": code,
        "err_code": err_code,
        "description": description,
    }))
    .unwrap()
}

fn stream_not_found() -> GetStreamError {
    GetStreamError::new(GetStreamErrorKind::JetStream(api_error(404, 10059, "stream not found")))
}

fn bucket_get_failed(source: GetStreamError) -> KeyValueError {
    KeyValueError::with_source(KeyValueErrorKind::GetBucket, source)
}

#[test]
fn stream_not_found_is_recognised() {
    assert!(is_get_stream_not_found(&stream_not_found()));
    assert!(is_get_key_value_not_found(&bucket_get_failed(stream_not_found())));
}

/// The distinction the callers depend on: a get that failed on the way to the
/// server says nothing about whether the resource exists, so provisioning must
/// not read it as absence and create over live storage.
#[test]
fn a_request_failure_is_not_absence() {
    let unreachable = GetStreamError::with_source(
        GetStreamErrorKind::Request,
        std::io::Error::other("no responders available"),
    );

    assert!(!is_get_stream_not_found(&unreachable));
    assert!(!is_get_key_value_not_found(&bucket_get_failed(unreachable)));
}

/// JetStream answers a great many things that are not "not found", and the ones
/// most likely to be met in production (an account without JetStream, a denied
/// API subject) are exactly the ones that must not trigger a create.
#[test]
fn another_jetstream_answer_is_not_absence() {
    for (code, err_code, description) in [
        (503, 10039, "jetstream not enabled for account"),
        (400, 10003, "bad request"),
        (500, 10008, "jetstream system temporarily unavailable"),
    ] {
        let error = GetStreamError::new(GetStreamErrorKind::JetStream(api_error(code, err_code, description)));

        assert!(!is_get_stream_not_found(&error), "{description}");
        assert!(!is_get_key_value_not_found(&bucket_get_failed(error)), "{description}");
    }
}

/// A bucket name the client rejected never reached the server, so nothing is
/// known about the bucket either way.
#[test]
fn a_client_side_rejection_is_not_absence() {
    assert!(!is_get_key_value_not_found(&KeyValueError::new(
        KeyValueErrorKind::InvalidStoreName
    )));
    assert!(!is_get_key_value_not_found(&KeyValueError::new(
        KeyValueErrorKind::JetStream
    )));
    assert!(!is_get_stream_not_found(&GetStreamError::new(
        GetStreamErrorKind::EmptyName
    )));
    assert!(!is_get_stream_not_found(&GetStreamError::new(
        GetStreamErrorKind::InvalidStreamName
    )));
}
