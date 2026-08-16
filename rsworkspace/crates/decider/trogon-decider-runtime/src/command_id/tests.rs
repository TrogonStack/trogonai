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
