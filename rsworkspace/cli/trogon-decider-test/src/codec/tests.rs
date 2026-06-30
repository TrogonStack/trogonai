use super::*;

#[test]
fn normalize_adds_prefix() {
    assert_eq!(
        normalize_type_url("trogonai.scheduler.schedules.v1.CreateSchedule"),
        "type.googleapis.com/trogonai.scheduler.schedules.v1.CreateSchedule"
    );
}

fn error_message<T>(result: Result<T>) -> String {
    match result {
        Ok(_) => String::new(),
        Err(error) => error.to_string(),
    }
}

#[test]
fn every_rejects_out_of_range_nanos() {
    let value = serde_json::json!({ "every": { "seconds": 1, "nanos": 1_000_000_000_i64 } });
    let error = error_message(parse_schedule(Some(&value)));
    assert!(error.contains("nanos"), "unexpected error: {error:?}");
}

#[test]
fn delivery_rejects_unsupported_ttl() {
    let value = serde_json::json!({ "subject": "agent.run", "ttl": "60s" });
    let error = error_message(parse_delivery(Some(&value)));
    assert!(error.contains("ttl"), "unexpected error: {error:?}");
}

#[test]
fn message_rejects_unsupported_headers() {
    let value = serde_json::json!({ "headers": [], "content": { "data": "{}" } });
    let error = error_message(parse_message(Some(&value)));
    assert!(error.contains("headers"), "unexpected error: {error:?}");
}
