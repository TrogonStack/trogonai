use std::io::{self, Write};

use buffa::bytes::Bytes;
use buffa::{HasMessageView, Message};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

#[cfg(feature = "schedules")]
use crate::scheduler::schedules::{checkpoints_v1, state_v1, v1};

struct InterruptedWriter {
    bytes: Vec<u8>,
    capacity: usize,
}

impl Write for InterruptedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let accepted = bytes.len().min(self.capacity - self.bytes.len());
        if accepted == 0 && !bytes.is_empty() {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        self.bytes.extend_from_slice(&bytes[..accepted]);
        Ok(accepted)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn assert_write_errors(value: &impl Serialize, expected: &Value) {
    let complete = serde_json::to_vec(value).expect("complete JSON serialization");
    assert_eq!(
        serde_json::from_slice::<Value>(&complete).expect("complete JSON value"),
        *expected
    );
    for capacity in 0..complete.len() {
        let mut writer = InterruptedWriter {
            bytes: Vec::new(),
            capacity,
        };
        let error = serde_json::to_writer(&mut writer, value).expect_err("interrupted JSON write");
        assert_eq!(error.io_error_kind(), Some(io::ErrorKind::BrokenPipe));
        assert_eq!(writer.bytes, complete[..capacity]);
    }
    let mut writer = InterruptedWriter {
        bytes: Vec::new(),
        capacity: complete.len(),
    };
    serde_json::to_writer(&mut writer, value).expect("exact-capacity JSON write");
    assert_eq!(writer.bytes, complete);
}

fn assert_view_write_errors<M>(expected: Value)
where
    M: Message + HasMessageView + DeserializeOwned,
    for<'a> M::View<'a>: Serialize,
    M::ViewHandle: Serialize,
{
    let message: M = serde_json::from_value(expected.clone()).expect("JSON fixture");
    let wire = message.encode_to_vec();
    let view = M::decode_view(&wire).expect("borrowed fixture");
    assert_write_errors(&view, &expected);
    drop(view);
    let retained = M::decode_view_handle(Bytes::from(wire)).expect("retained fixture");
    assert_write_errors(&retained, &expected);
}

#[cfg(feature = "schedules")]
#[test]
fn civil_datetime_views_propagate_write_errors_in_every_coordinate() {
    assert_view_write_errors::<crate::google::r#type::DateTime>(json!({
        "year": 2026, "month": 9, "day": 5, "hours": 11,
        "minutes": 12, "seconds": 13, "nanos": 123456789,
        "timeZone": {"id": "UTC"}
    }));
}

#[cfg(any(feature = "decider", feature = "grpc-nats-micro"))]
#[test]
fn error_detail_views_preserve_transport_write_failures() {
    assert_view_write_errors::<crate::google::rpc::quota_failure::Violation>(json!({
        "quotaValue": "9007199254740993", "futureQuotaValue": "9007199254740994"
    }));
    assert_view_write_errors::<crate::google::rpc::Status>(json!({
        "code": 5, "message": "schedule missing",
        "details": [{"@type": crate::google::rpc::ErrorInfo::TYPE_URL, "value": "CgF4"}]
    }));
}

#[cfg(feature = "decider")]
#[test]
fn decider_views_propagate_write_errors_in_revision_and_position() {
    assert_view_write_errors::<crate::decider::v1::DecideRequest>(json!({
        "command": {}, "commandId": "command-7", "expectedRevision": "9007199254740993"
    }));
    assert_view_write_errors::<crate::decider::v1::DecideResponse>(json!({
        "streamPosition": "9007199254740993"
    }));
}

#[cfg(feature = "schedules")]
#[test]
fn scheduler_views_propagate_write_errors_without_losing_sequence_presence() {
    assert_view_write_errors::<checkpoints_v1::ScheduleCheckpoint>(json!({
        "scheduleId": "backup", "lastAppliedStreamPosition": "9007199254740993"
    }));
    assert_view_write_errors::<state_v1::State>(json!({"lastOccurrenceSequence": "9007199254740993"}));
    assert_view_write_errors::<v1::ScheduleCompleted>(json!({
        "scheduleId": "backup", "lastOccurrenceSequence": "9007199254740993"
    }));
    assert_view_write_errors::<v1::ScheduleOccurrenceRecorded>(json!({
        "scheduleId": "backup", "occurrenceSequence": "9007199254740993",
        "occurrenceAt": "1970-01-01T00:00:10Z", "recordedAt": "1970-01-01T00:00:11Z"
    }));
    assert_view_write_errors::<v1::ScheduleOccurrenceScheduled>(json!({
        "scheduleId": "backup", "occurrenceSequence": "9007199254740993",
        "occurrenceAt": "1970-01-01T00:00:10Z", "scheduledAt": "1970-01-01T00:00:09Z"
    }));
}
