use buffa::MessageField;
use buffa_types::google::protobuf::Timestamp;
use chrono::{DateTime, Utc};

pub fn proto_timestamp(dt: &DateTime<Utc>) -> MessageField<Timestamp> {
    MessageField::some(Timestamp::from_unix(dt.timestamp(), dt.timestamp_subsec_nanos() as i32))
}

pub fn proto_timestamp_rfc3339(timestamp: &str) -> Result<MessageField<Timestamp>, chrono::ParseError> {
    let dt = DateTime::parse_from_rfc3339(timestamp)?.with_timezone(&Utc);
    Ok(proto_timestamp(&dt))
}
