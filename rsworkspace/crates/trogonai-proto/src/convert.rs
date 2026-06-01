use buffa_types::google::protobuf::{Duration, Timestamp};
use chrono::{DateTime, Utc};

pub const PROTOBUF_DURATION_MAX_SECONDS: u64 = 315_576_000_000;
pub const PROTOBUF_DURATION_MAX: std::time::Duration = std::time::Duration::from_secs(PROTOBUF_DURATION_MAX_SECONDS);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DurationConversionError {
    max: std::time::Duration,
    actual: std::time::Duration,
}

impl DurationConversionError {
    pub fn max(self) -> std::time::Duration {
        self.max
    }

    pub fn actual(self) -> std::time::Duration {
        self.actual
    }
}

impl std::fmt::Display for DurationConversionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "duration must fit the protobuf Duration range: max {:?}, got {:?}",
            self.max, self.actual
        )
    }
}

impl std::error::Error for DurationConversionError {}

pub fn timestamp_from_datetime(value: &DateTime<Utc>) -> Timestamp {
    Timestamp::from_unix(value.timestamp(), value.timestamp_subsec_nanos() as i32)
}

pub fn duration_from_std(value: std::time::Duration) -> Result<Duration, DurationConversionError> {
    if value > PROTOBUF_DURATION_MAX {
        return Err(DurationConversionError {
            max: PROTOBUF_DURATION_MAX,
            actual: value,
        });
    }

    let Ok(seconds) = i64::try_from(value.as_secs()) else {
        return Err(DurationConversionError {
            max: PROTOBUF_DURATION_MAX,
            actual: value,
        });
    };
    let Ok(nanos) = i32::try_from(value.subsec_nanos()) else {
        return Err(DurationConversionError {
            max: PROTOBUF_DURATION_MAX,
            actual: value,
        });
    };

    Ok(Duration::from_secs_nanos(seconds, nanos))
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
        let duration = duration_from_std(std::time::Duration::new(30, 123_456_789)).unwrap();

        assert_eq!(duration.seconds, 30);
        assert_eq!(duration.nanos, 123_456_789);
    }

    #[test]
    fn duration_from_std_preserves_max_whole_seconds() {
        let duration = duration_from_std(PROTOBUF_DURATION_MAX).unwrap();

        assert_eq!(duration.seconds, 315_576_000_000);
        assert_eq!(duration.nanos, 0);
    }

    #[test]
    fn duration_from_std_reports_values_outside_protobuf_range() {
        let err = duration_from_std(PROTOBUF_DURATION_MAX + std::time::Duration::from_nanos(1)).unwrap_err();

        assert_eq!(err.max(), PROTOBUF_DURATION_MAX);
        assert_eq!(err.actual(), PROTOBUF_DURATION_MAX + std::time::Duration::from_nanos(1));
        assert_eq!(
            err.to_string(),
            "duration must fit the protobuf Duration range: max 315576000000s, got 315576000000.000000001s"
        );
    }
}
