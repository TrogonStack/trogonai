use buffa::Message as _;
use trogonai_proto::scheduler::schedules::v1 as schedules_v1;

use super::*;

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
    let command = json_any_to_command(&create_schedule_value()).expect("encode command");
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
    let envelope = json_any_to_envelope(&value).expect("encode envelope");
    assert_eq!(envelope.type_url, "trogonai.scheduler.schedules.v1.SchedulePaused");

    let decoded = schedules_v1::SchedulePaused::decode_from_slice(&envelope.payload).expect("decode envelope");
    assert_eq!(decoded.schedule_id, "backup");
}

#[test]
fn missing_type_is_an_error() {
    let value = serde_json::json!({ "schedule_id": "backup" });
    let error = json_any_to_command(&value).unwrap_err().to_string();
    assert!(error.contains("@type"), "unexpected error: {error}");
}

#[test]
fn unregistered_type_is_an_error() {
    let value = serde_json::json!({ "@type": "type.googleapis.com/trogonai.scheduler.schedules.v1.NoSuchType" });
    let error = json_any_to_command(&value).unwrap_err().to_string();
    assert!(error.contains("unregistered"), "unexpected error: {error}");
}
