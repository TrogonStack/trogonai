use buffa_types::google::protobuf::{Duration, Timestamp};
use chrono::{DateTime, Utc};

pub const PROTOBUF_DURATION_MAX_SECONDS: u64 = 315_576_000_000;
pub const PROTOBUF_DURATION_MAX: std::time::Duration = std::time::Duration::from_secs(PROTOBUF_DURATION_MAX_SECONDS);

pub fn timestamp_from_datetime(value: &DateTime<Utc>) -> Timestamp {
    Timestamp::from_unix(value.timestamp(), value.timestamp_subsec_nanos() as i32)
}

pub fn duration_from_std(value: std::time::Duration) -> Duration {
    assert!(
        value <= PROTOBUF_DURATION_MAX,
        "duration must fit the protobuf Duration range"
    );

    let seconds = i64::try_from(value.as_secs())
        .ok()
        .expect("duration must fit the protobuf Duration range");

    Duration::from_secs_nanos(seconds, value.subsec_nanos() as i32)
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    #[test]
    fn timestamp_from_datetime_preserves_epoch_seconds_and_nanos() {
        let timestamp = timestamp_from_datetime(&Utc.timestamp_opt(1_451_600_400, 123_456_789).unwrap());

        assert_eq!(timestamp.seconds, 1_451_600_400);
        assert_eq!(timestamp.nanos, 123_456_789);
    }

    #[test]
    fn timestamp_from_datetime_handles_negative_seconds_with_positive_nanos() {
        let timestamp = timestamp_from_datetime(&Utc.timestamp_opt(-1, 250_000_000).unwrap());

        assert_eq!(timestamp.seconds, -1);
        assert_eq!(timestamp.nanos, 250_000_000);
    }

    #[test]
    fn duration_from_std_preserves_duration_parts_inside_protobuf_range() {
        let duration = duration_from_std(std::time::Duration::new(30, 123_456_789));

        assert_eq!(duration.seconds, 30);
        assert_eq!(duration.nanos, 123_456_789);
    }

    #[test]
    fn duration_from_std_preserves_max_whole_seconds() {
        let duration = duration_from_std(PROTOBUF_DURATION_MAX);

        assert_eq!(duration.seconds, 315_576_000_000);
        assert_eq!(duration.nanos, 0);
    }

    #[test]
    #[should_panic(expected = "duration must fit the protobuf Duration range")]
    fn duration_from_std_panics_outside_protobuf_range() {
        let _ = duration_from_std(PROTOBUF_DURATION_MAX + std::time::Duration::from_nanos(1));
    }
}
