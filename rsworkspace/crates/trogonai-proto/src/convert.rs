use buffa_types::google::protobuf::{Duration, Timestamp};
use chrono::{DateTime, Utc};

pub fn timestamp_from_datetime(value: &DateTime<Utc>) -> Timestamp {
    Timestamp::from_unix(value.timestamp(), value.timestamp_subsec_nanos() as i32)
}

pub fn duration_from_seconds(seconds: u64) -> Duration {
    Duration::from_secs(i64::try_from(seconds).unwrap_or(i64::MAX))
}
