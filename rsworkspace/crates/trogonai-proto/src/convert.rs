use buffa_types::google::protobuf::{Duration, Timestamp};
use chrono::{DateTime, Utc};

pub const PROTOBUF_DURATION_MAX_SECONDS: u64 = 315_576_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ProtobufDurationSeconds {
    seconds: i64,
}

impl ProtobufDurationSeconds {
    pub fn new(seconds: u64) -> Option<Self> {
        let seconds = i64::try_from(seconds).ok()?;
        if seconds > PROTOBUF_DURATION_MAX_SECONDS as i64 {
            return None;
        }

        Some(Self { seconds })
    }

    pub fn as_i64(self) -> i64 {
        self.seconds
    }

    pub fn as_u64(self) -> u64 {
        self.seconds as u64
    }
}

pub fn timestamp_from_datetime(value: &DateTime<Utc>) -> Timestamp {
    Timestamp::from_unix(value.timestamp(), value.timestamp_subsec_nanos() as i32)
}

pub fn duration_from_seconds(seconds: ProtobufDurationSeconds) -> Duration {
    Duration::from_secs(seconds.as_i64())
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
    fn duration_from_seconds_preserves_whole_seconds_inside_protobuf_range() {
        let seconds = ProtobufDurationSeconds::new(PROTOBUF_DURATION_MAX_SECONDS).unwrap();
        let duration = duration_from_seconds(seconds);

        assert_eq!(duration.seconds, 315_576_000_000);
        assert_eq!(duration.nanos, 0);
    }

    #[test]
    fn protobuf_duration_seconds_rejects_values_outside_protobuf_range() {
        assert!(ProtobufDurationSeconds::new(PROTOBUF_DURATION_MAX_SECONDS + 1).is_none());
    }
}
