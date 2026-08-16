use uuid::Uuid;

use super::*;

const CREATE_SCHEDULE: &str = "type.googleapis.com/trogonai.scheduler.schedules.v1.CreateSchedule";
const COMMAND_UUID: &str = "0198be07-a384-79e1-a376-f250f9181bec";

fn command_type() -> CommandType {
    CommandType::new(CREATE_SCHEDULE).expect("test command type is valid")
}

fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in pairs {
        headers.insert(*name, *value);
    }
    headers
}

#[test]
fn a_bare_request_carries_the_subject_type_and_the_payload() {
    let request = CommandRequest::parse(&command_type(), vec![1, 2, 3], None).expect("a headerless request parses");

    assert_eq!(request.command().type_, CREATE_SCHEDULE);
    assert_eq!(request.command().payload, vec![1, 2, 3]);
    assert_eq!(request.command_id(), None);
    assert_eq!(request.expected_revision(), None);
}

#[test]
fn a_protobuf_content_type_is_accepted() {
    CommandRequest::parse(
        &command_type(),
        Vec::new(),
        Some(&headers(&[(CONTENT_TYPE_HEADER, PROTOBUF_CONTENT_TYPE)])),
    )
    .expect("the declared encoding is the one this binding defines");
}

#[test]
fn another_content_type_is_refused_rather_than_reinterpreted() {
    let error = CommandRequest::parse(
        &command_type(),
        Vec::new(),
        Some(&headers(&[(CONTENT_TYPE_HEADER, "application/json")])),
    )
    .expect_err("json is not an encoding this binding implements");

    assert!(
        matches!(error, CommandRequestError::UnsupportedContentType { .. }),
        "decoding json bytes as protobuf would hand the guest garbage instead of an error: {error}"
    );
}

#[test]
fn a_command_id_header_becomes_the_command_identity() {
    let request = CommandRequest::parse(
        &command_type(),
        Vec::new(),
        Some(&headers(&[(TROGON_COMMAND_ID_HEADER, COMMAND_UUID)])),
    )
    .expect("a uuid header parses");

    assert_eq!(
        request.command_id().map(CommandId::as_uuid),
        Some(Uuid::parse_str(COMMAND_UUID).expect("the fixture is a uuid"))
    );
}

#[test]
fn an_unparseable_command_id_fails_the_command_instead_of_being_dropped() {
    let error = CommandRequest::parse(
        &command_type(),
        Vec::new(),
        Some(&headers(&[(TROGON_COMMAND_ID_HEADER, "not-a-uuid")])),
    )
    .expect_err("a malformed idempotency key is not an absent one");

    assert!(
        matches!(error, CommandRequestError::CommandId { .. }),
        "ignoring it would leave the caller believing its retries are deduplicated when they are not: {error}"
    );
}

#[test]
fn an_expected_revision_header_becomes_a_stream_position() {
    let request = CommandRequest::parse(
        &command_type(),
        Vec::new(),
        Some(&headers(&[(TROGON_EXPECTED_REVISION_HEADER, "7")])),
    )
    .expect("a decimal revision parses");

    assert_eq!(request.expected_revision().map(StreamPosition::as_u64), Some(7));
}

#[test]
fn a_non_numeric_expected_revision_fails_the_command() {
    let error = CommandRequest::parse(
        &command_type(),
        Vec::new(),
        Some(&headers(&[(TROGON_EXPECTED_REVISION_HEADER, "seven")])),
    )
    .expect_err("a revision must be a number");

    assert!(
        matches!(error, CommandRequestError::ExpectedRevisionNotANumber { .. }),
        "{error}"
    );
}

#[test]
fn a_zero_expected_revision_fails_the_command() {
    let error = CommandRequest::parse(
        &command_type(),
        Vec::new(),
        Some(&headers(&[(TROGON_EXPECTED_REVISION_HEADER, "0")])),
    )
    .expect_err("zero is not a revision a caller gets to assert");

    assert!(
        matches!(error, CommandRequestError::ExpectedRevisionZero),
        "an empty stream is the module's no_stream precondition, not a caller-supplied guard: {error}"
    );
}

#[test]
fn a_negative_expected_revision_fails_the_command() {
    let error = CommandRequest::parse(
        &command_type(),
        Vec::new(),
        Some(&headers(&[(TROGON_EXPECTED_REVISION_HEADER, "-1")])),
    )
    .expect_err("a revision counts events, so it is never negative");

    assert!(
        matches!(error, CommandRequestError::ExpectedRevisionNotANumber { .. }),
        "{error}"
    );
}
