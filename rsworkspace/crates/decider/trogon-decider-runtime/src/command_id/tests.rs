use uuid::Uuid;

use super::CommandId;

fn command_id() -> CommandId {
    CommandId::new(Uuid::parse_str("0198be07-a384-79e1-a376-f250f9181be9").expect("valid uuid"))
}

#[test]
fn the_same_command_derives_the_same_event_ids_on_every_delivery() {
    let first_attempt: Vec<_> = (0..3).map(|index| command_id().event_id(index)).collect();
    let redelivery: Vec<_> = (0..3).map(|index| command_id().event_id(index)).collect();

    assert_eq!(first_attempt, redelivery);
}

#[test]
fn each_event_in_one_batch_gets_a_distinct_id() {
    let id = command_id();

    assert_ne!(id.event_id(0), id.event_id(1));
    assert_ne!(id.event_id(1), id.event_id(2));
}

#[test]
fn different_commands_never_share_an_event_id() {
    let other = CommandId::new(Uuid::parse_str("0198be07-a384-79e1-a376-f250f9181bee").expect("valid uuid"));

    assert_ne!(command_id().event_id(0), other.event_id(0));
}

#[test]
fn a_derived_id_depends_on_both_the_namespace_and_the_key() {
    let namespace = Uuid::parse_str("0198be07-a384-79e1-a376-f250f9181be9").expect("valid uuid");
    let other_namespace = Uuid::parse_str("0198be07-a384-79e1-a376-f250f9181bee").expect("valid uuid");

    assert_eq!(
        CommandId::derive(&namespace, b"key"),
        CommandId::derive(&namespace, b"key")
    );
    assert_ne!(
        CommandId::derive(&namespace, b"key"),
        CommandId::derive(&namespace, b"other")
    );
    assert_ne!(
        CommandId::derive(&namespace, b"key"),
        CommandId::derive(&other_namespace, b"key")
    );
}

#[test]
fn a_command_id_round_trips_through_its_string_form() {
    let parsed: CommandId = command_id().to_string().parse().expect("a rendered id reparses");

    assert_eq!(parsed, command_id());
    assert_eq!(parsed.as_uuid(), command_id().as_uuid());
}

#[test]
fn a_command_id_round_trips_through_its_serialized_form() {
    let encoded = serde_json::to_string(&command_id()).expect("a command id serializes");

    assert_eq!(
        encoded, "\"0198be07-a384-79e1-a376-f250f9181be9\"",
        "a stored id is written as the string an operator can match against a log line, not as bytes"
    );
    assert_eq!(
        serde_json::from_str::<CommandId>(&encoded).expect("its own form deserializes"),
        command_id(),
        "an id that did not survive a round trip would break the idempotency the caller was promised"
    );
}

#[test]
fn a_serialized_id_that_is_not_a_uuid_is_refused() {
    let error = serde_json::from_str::<CommandId>("\"not-a-uuid\"").expect_err("that is not an id");

    assert!(
        error.to_string().contains("invalid character"),
        "a decoder that accepted it would hand the runtime an id no derivation can be trusted against: {error}"
    );
}

#[test]
fn a_command_id_converts_both_ways_with_the_uuid_it_wraps() {
    let uuid = Uuid::parse_str("0198be07-a384-79e1-a376-f250f9181be9").expect("valid uuid");

    assert_eq!(CommandId::from(uuid), command_id());
    assert_eq!(Uuid::from(command_id()), uuid);
}

#[test]
fn a_command_id_names_itself_in_a_diagnostic() {
    assert_eq!(
        format!("{:?}", command_id()),
        "CommandId(0198be07-a384-79e1-a376-f250f9181be9)"
    );
}
