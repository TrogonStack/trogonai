use buffa_types::google::protobuf::{Duration, Timestamp};
use chrono::{DateTime, Utc};

pub const PROTOBUF_DURATION_MAX_SECONDS: u64 = 315_576_000_000;
pub const PROTOBUF_DURATION_MAX: std::time::Duration = std::time::Duration::from_secs(PROTOBUF_DURATION_MAX_SECONDS);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("duration must fit the protobuf Duration range: max {max:?}, got {actual:?}")]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("timestamp is outside the representable datetime range: seconds {seconds}, nanos {nanos}")]
pub struct TimestampConversionError {
    seconds: i64,
    nanos: i32,
}

impl TimestampConversionError {
    pub fn seconds(self) -> i64 {
        self.seconds
    }

    pub fn nanos(self) -> i32 {
        self.nanos
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("duration must be non-negative with nanos below one second: seconds {seconds}, nanos {nanos}")]
pub struct StdDurationConversionError {
    seconds: i64,
    nanos: i32,
}

impl StdDurationConversionError {
    pub fn seconds(self) -> i64 {
        self.seconds
    }

    pub fn nanos(self) -> i32 {
        self.nanos
    }
}

pub fn timestamp_from_datetime(value: &DateTime<Utc>) -> Timestamp {
    Timestamp::from_unix(value.timestamp(), value.timestamp_subsec_nanos() as i32)
}

pub fn datetime_from_timestamp(value: &Timestamp) -> Result<DateTime<Utc>, TimestampConversionError> {
    u32::try_from(value.nanos)
        .ok()
        .and_then(|nanos| DateTime::<Utc>::from_timestamp(value.seconds, nanos))
        .ok_or(TimestampConversionError {
            seconds: value.seconds,
            nanos: value.nanos,
        })
}

pub fn std_from_duration(value: &Duration) -> Result<std::time::Duration, StdDurationConversionError> {
    let error = StdDurationConversionError {
        seconds: value.seconds,
        nanos: value.nanos,
    };
    let seconds = u64::try_from(value.seconds).map_err(|_| error)?;
    let nanos = u32::try_from(value.nanos).map_err(|_| error)?;
    if nanos >= 1_000_000_000 {
        return Err(error);
    }

    Ok(std::time::Duration::new(seconds, nanos))
}

pub fn duration_from_std(value: std::time::Duration) -> Result<Duration, DurationConversionError> {
    if value > PROTOBUF_DURATION_MAX {
        return Err(DurationConversionError {
            max: PROTOBUF_DURATION_MAX,
            actual: value,
        });
    }

    let seconds = value.as_secs() as i64;
    let nanos = value.subsec_nanos() as i32;

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
    fn datetime_from_timestamp_round_trips_epoch_seconds_and_nanos() {
        let datetime = datetime_from_timestamp(&Timestamp::from_unix(1_451_600_400, 123_456_789)).unwrap();

        assert_eq!(datetime, Utc.timestamp_opt(1_451_600_400, 123_456_789).unwrap());
    }

    #[test]
    fn datetime_from_timestamp_reports_negative_nanos() {
        let mut timestamp = Timestamp::from_unix(0, 0);
        timestamp.nanos = -1;
        let err = datetime_from_timestamp(&timestamp).unwrap_err();

        assert_eq!(err.seconds(), 0);
        assert_eq!(err.nanos(), -1);
        assert_eq!(
            err.to_string(),
            "timestamp is outside the representable datetime range: seconds 0, nanos -1"
        );
    }

    #[test]
    fn datetime_from_timestamp_reports_out_of_range_seconds() {
        let err = datetime_from_timestamp(&Timestamp::from_unix(i64::MAX, 0)).unwrap_err();

        assert_eq!(err.seconds(), i64::MAX);
    }

    #[test]
    fn std_from_duration_preserves_duration_parts() {
        let duration = std_from_duration(&Duration::from_secs_nanos(30, 123_456_789)).unwrap();

        assert_eq!(duration, std::time::Duration::new(30, 123_456_789));
    }

    #[test]
    fn std_from_duration_reports_negative_seconds() {
        let err = std_from_duration(&Duration::from_secs_nanos(-1, 0)).unwrap_err();

        assert_eq!(err.seconds(), -1);
        assert_eq!(err.nanos(), 0);
        assert_eq!(
            err.to_string(),
            "duration must be non-negative with nanos below one second: seconds -1, nanos 0"
        );
    }

    #[test]
    fn std_from_duration_reports_negative_nanos() {
        let mut duration = Duration::from_secs_nanos(1, 0);
        duration.nanos = -1;
        let err = std_from_duration(&duration).unwrap_err();

        assert_eq!(err.nanos(), -1);
    }

    #[test]
    fn std_from_duration_reports_nanos_of_a_second_or_more() {
        let mut duration = Duration::from_secs_nanos(1, 0);
        duration.nanos = 1_000_000_000;
        let err = std_from_duration(&duration).unwrap_err();

        assert_eq!(err.nanos(), 1_000_000_000);
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
