use buffa::MessageField;
use buffa_types::google::protobuf::Any;
use uuid::Uuid;

use super::*;

const CREATE_SCHEDULE: &str = "type.googleapis.com/trogonai.scheduler.schedules.v1.CreateSchedule";
const COMMAND_UUID: &str = "0198be07-a384-79e1-a376-f250f9181bec";

fn request(payload: Vec<u8>) -> DecideRequest {
    DecideRequest {
        command: MessageField::some(Any {
            type_url: CREATE_SCHEDULE.to_owned(),
            value: payload.into(),
            ..Any::default()
        }),
        ..DecideRequest::default()
    }
}

fn parse(request: &DecideRequest) -> Result<CommandRequest, CommandRequestError> {
    CommandRequest::parse(&request.encode_to_vec(), None)
}

fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
    let mut headers = HeaderMap::new();
    for (name, value) in pairs {
        headers.insert(*name, *value);
    }
    headers
}

#[test]
fn a_bare_request_carries_the_command_type_and_the_payload() {
    let parsed = parse(&request(vec![1, 2, 3])).expect("a minimal request parses");

    assert_eq!(
        parsed.command().type_,
        CREATE_SCHEDULE,
        "the type url the caller sent is what the guest is handed, so a host that rewrote it would route one command and execute another"
    );
    assert_eq!(parsed.command().payload, vec![1, 2, 3]);
    assert_eq!(parsed.command_id(), None);
    assert_eq!(parsed.expected_revision(), None);
}

#[test]
fn a_protobuf_content_type_is_accepted() {
    CommandRequest::parse(
        &request(Vec::new()).encode_to_vec(),
        Some(&headers(&[(CONTENT_TYPE_HEADER, PROTOBUF_CONTENT_TYPE)])),
    )
    .expect("the declared encoding is the one this endpoint accepts");
}

#[test]
fn another_content_type_is_refused_rather_than_reinterpreted() {
    let error = CommandRequest::parse(
        &request(Vec::new()).encode_to_vec(),
        Some(&headers(&[(CONTENT_TYPE_HEADER, "application/json")])),
    )
    .expect_err("json is not an encoding DeciderService declares");

    assert!(
        matches!(error, CommandRequestError::UnsupportedContentType { .. }),
        "decoding json bytes as protobuf would hand the guest garbage instead of an error: {error}"
    );
}

#[test]
fn a_payload_that_is_not_a_decide_request_fails_the_command() {
    let error =
        CommandRequest::parse(&[0xff, 0xff, 0xff, 0xff], None).expect_err("those bytes are not a DecideRequest");

    assert!(
        matches!(error, CommandRequestError::Undecodable { .. }),
        "a caller that sent an unreadable envelope has to learn that, rather than learn its command was unroutable and go audit a deployment that is fine: {error}"
    );
}

#[test]
fn a_request_naming_no_command_fails_rather_than_executing_an_empty_one() {
    let error = parse(&DecideRequest::default()).expect_err("there is no command to route");

    assert!(matches!(error, CommandRequestError::NoCommand), "{error}");
}

#[test]
fn a_type_url_that_is_not_a_command_type_fails_the_command() {
    let mut malformed = request(Vec::new());
    malformed.command = MessageField::some(Any {
        type_url: String::new(),
        ..Any::default()
    });

    let error = parse(&malformed).expect_err("an empty type url names nothing");

    assert!(matches!(error, CommandRequestError::CommandType { .. }), "{error}");
}

#[test]
fn a_command_id_becomes_the_command_identity() {
    let parsed = parse(&request(Vec::new()).with_command_id(COMMAND_UUID.to_owned())).expect("a uuid parses");

    assert_eq!(
        parsed.command_id().map(CommandId::as_uuid),
        Some(Uuid::parse_str(COMMAND_UUID).expect("the fixture is a uuid"))
    );
}

#[test]
fn an_unparseable_command_id_fails_the_command_instead_of_being_dropped() {
    let error =
        parse(&request(Vec::new()).with_command_id("not-a-uuid".to_owned())).expect_err("that is not an identity");

    assert!(
        matches!(error, CommandRequestError::CommandId { .. }),
        "ignoring it would leave the caller believing its retries are deduplicated when they are not: {error}"
    );
}

#[test]
fn an_expected_revision_becomes_a_stream_position() {
    let parsed = parse(&request(Vec::new()).with_expected_revision(7)).expect("a revision parses");

    assert_eq!(parsed.expected_revision().map(StreamPosition::as_u64), Some(7));
}

#[test]
fn a_zero_expected_revision_fails_the_command() {
    let error = parse(&request(Vec::new()).with_expected_revision(0)).expect_err("zero is not a revision to assert");

    assert!(
        matches!(error, CommandRequestError::ExpectedRevisionZero),
        "an empty stream is the module's no_stream precondition, not a caller-supplied guard: {error}"
    );
}
