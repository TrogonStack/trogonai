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
mod tests;
