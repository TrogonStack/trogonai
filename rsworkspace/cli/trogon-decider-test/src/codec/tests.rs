use buffa::Message as _;
use trogonai_proto::scheduler::schedules::v1 as schedules_v1;

use super::*;

fn schedules_registry() -> &'static TypeRegistry {
    type_registry("scheduler.schedules").expect("scheduler.schedules is registered")
}

#[test]
fn normalize_adds_prefix() {
    assert_eq!(
        normalize_type_url("trogonai.scheduler.schedules.v1.CreateSchedule"),
        "type.googleapis.com/trogonai.scheduler.schedules.v1.CreateSchedule"
    );
}

fn create_schedule_value() -> serde_json::Value {
    serde_json::json!({
        "@type": "type.googleapis.com/trogonai.scheduler.schedules.v1.CreateSchedule",
        "schedule_id": "backup",
        "status": { "scheduled": {} },
        "schedule": { "every": { "every": "30s" } },
        "delivery": { "nats_message": { "subject": "agent.run" } },
        "message": { "content": { "content_type": "application/json", "data": "e30=" } },
    })
}

#[test]
fn json_any_to_command_encodes_via_registry() {
    let command = json_any_to_command(schedules_registry(), &create_schedule_value()).expect("encode command");
    assert_eq!(
        command.type_url,
        "type.googleapis.com/trogonai.scheduler.schedules.v1.CreateSchedule"
    );

    let decoded = schedules_v1::CreateSchedule::decode_from_slice(&command.payload).expect("decode command");
    assert_eq!(decoded.schedule_id, "backup");

    let round_tripped_text =
        trogonai_proto::decode_event_to_json("trogonai.scheduler.schedules.v1.CreateSchedule", &command.payload)
            .expect("registered type")
            .expect("decode to json");
    let round_tripped: serde_json::Value = serde_json::from_str(&round_tripped_text).expect("valid json");
    assert_eq!(round_tripped["delivery"]["natsMessage"]["subject"], "agent.run");
}

#[test]
fn json_any_to_envelope_uses_bare_full_name() {
    let value = serde_json::json!({
        "@type": "trogonai.scheduler.schedules.v1.SchedulePaused",
        "schedule_id": "backup",
    });
    let envelope = json_any_to_envelope(schedules_registry(), &value).expect("encode envelope");
    assert_eq!(envelope.type_url, "trogonai.scheduler.schedules.v1.SchedulePaused");

    let decoded = schedules_v1::SchedulePaused::decode_from_slice(&envelope.payload).expect("decode envelope");
    assert_eq!(decoded.schedule_id, "backup");
}

#[test]
fn utf8_wrapper_encodes_identically_to_base64() {
    let mut wrapped = create_schedule_value();
    wrapped["message"]["content"]["data"] = serde_json::json!({ "utf8": "{}" });

    let from_wrapper = json_any_to_command(schedules_registry(), &wrapped).expect("encode wrapped command");
    let from_base64 =
        json_any_to_command(schedules_registry(), &create_schedule_value()).expect("encode base64 command");
    assert_eq!(from_wrapper.payload, from_base64.payload);

    let decoded = schedules_v1::CreateSchedule::decode_from_slice(&from_wrapper.payload).expect("decode command");
    let content = decoded.message.expect("message").content.expect("content");
    assert_eq!(content.data, b"{}");
}

#[test]
fn utf8_payload_expansion_preserves_repeated_message_headers() {
    let mut value = create_schedule_value();
    value["message"]["content"]["data"] = serde_json::json!({ "utf8": "scheduled café" });
    value["message"]["headers"] = serde_json::json!([
        { "name": "X-Workflow", "value": "nightly" },
        { "name": "X-Region", "value": "west" },
    ]);

    let encoded = json_any_to_command(schedules_registry(), &value).unwrap();
    let decoded = schedules_v1::CreateSchedule::decode_from_slice(&encoded.payload).unwrap();
    let message = decoded.message.unwrap();

    assert_eq!(message.content.unwrap().data, "scheduled café".as_bytes());
    assert_eq!(
        message
            .headers
            .iter()
            .map(|header| (header.name.as_str(), header.value.as_str()))
            .collect::<Vec<_>>(),
        vec![("X-Workflow", "nightly"), ("X-Region", "west")]
    );
}

#[test]
fn missing_type_is_an_error() {
    let value = serde_json::json!({ "schedule_id": "backup" });
    let error = json_any_to_command(schedules_registry(), &value)
        .unwrap_err()
        .to_string();
    assert!(error.contains("@type"), "unexpected error: {error}");
}

#[test]
fn unregistered_type_is_an_error() {
    let value = serde_json::json!({ "@type": "type.googleapis.com/trogonai.scheduler.schedules.v1.NoSuchType" });
    let error = json_any_to_command(schedules_registry(), &value)
        .unwrap_err()
        .to_string();
    assert!(error.contains("unregistered"), "unexpected error: {error}");
}

#[test]
fn type_registry_reports_unknown_module() {
    let error = type_registry("not.a.decider").unwrap_err().to_string();
    assert!(error.contains("no decider registered"), "unexpected error: {error}");
}

#[test]
fn declared_events_lists_the_schedules_event_set() {
    let events = declared_events("scheduler.schedules").expect("scheduler.schedules is registered");
    assert!(events.contains(&"trogonai.scheduler.schedules.v1.ScheduleCreated"));
    assert!(events.contains(&"trogonai.scheduler.schedules.v1.SchedulePaused"));
    assert!(events.contains(&"trogonai.scheduler.schedules.v1.ScheduleResumed"));
    assert!(events.contains(&"trogonai.scheduler.schedules.v1.ScheduleRemoved"));
}

#[test]
fn declared_events_reports_unknown_module() {
    let error = declared_events("not.a.decider").unwrap_err().to_string();
    assert!(error.contains("no decider registered"), "unexpected error: {error}");
}
