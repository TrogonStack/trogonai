use async_nats::{HeaderMap, header::NATS_MESSAGE_ID, jetstream};
use buffa::{Message, MessageName};
use trogon_decider_nats::TROGON_EVENT_TYPE;
use trogon_scheduler::{constants::EVENTS_SUBJECT_PREFIX, v1};
use trogon_std::{NowV7, UuidV7Generator};

pub async fn publish_anomalies(js: &jetstream::Context, absent_id: &str, existing_id: &str) {
    let paused = v1::SchedulePaused {
        schedule_id: absent_id.to_string(),
    };
    let misrouted = v1::ScheduleRemoved {
        schedule_id: existing_id.to_string(),
    };
    for (route, event_type, payload) in [
        (absent_id, "example.future.Event", Vec::new()),
        (absent_id, v1::SchedulePaused::FULL_NAME, vec![0xff]),
        (absent_id, v1::SchedulePaused::FULL_NAME, paused.encode_to_vec()),
        (absent_id, v1::ScheduleRemoved::FULL_NAME, misrouted.encode_to_vec()),
        ("invalid-id", v1::SchedulePaused::FULL_NAME, paused.encode_to_vec()),
    ] {
        let mut headers = HeaderMap::new();
        headers.insert(NATS_MESSAGE_ID, UuidV7Generator.now_v7().to_string());
        headers.insert(TROGON_EVENT_TYPE, event_type);
        js.publish_with_headers(format!("{EVENTS_SUBJECT_PREFIX}{route}"), headers, payload.into())
            .await
            .unwrap()
            .await
            .unwrap();
    }
}
