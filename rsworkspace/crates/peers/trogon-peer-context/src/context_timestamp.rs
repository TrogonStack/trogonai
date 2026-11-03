use buffa::json_helpers::wkt::{MAX_TIMESTAMP_SECS, MIN_TIMESTAMP_SECS};
use buffa_types::google::protobuf::Timestamp;
use serde_json::Value;

use crate::PeerContextError;

pub(crate) struct ContextTimestamp<'a>(&'a Timestamp);

impl<'a> ContextTimestamp<'a> {
    pub(crate) fn from_wire(timestamp: &'a Timestamp) -> Result<Self, PeerContextError> {
        if !(MIN_TIMESTAMP_SECS..=MAX_TIMESTAMP_SECS).contains(&timestamp.seconds)
            || !(0..1_000_000_000).contains(&timestamp.nanos)
        {
            return Err(PeerContextError::ContractViolation);
        }
        Ok(Self(timestamp))
    }

    pub(crate) fn to_json(&self) -> Result<Value, PeerContextError> {
        serde_json::to_value(self.0).map_err(PeerContextError::Encode)
    }
}
