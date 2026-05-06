use serde::{Serialize, de::DeserializeOwned};

use super::EventCodec;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JsonEventCodec;

impl<T> EventCodec<T> for JsonEventCodec
where
    T: Serialize + DeserializeOwned,
{
    type Error = serde_json::Error;

    fn encode(&self, value: &T) -> Result<Vec<u8>, Self::Error> {
        serde_json::to_vec(value)
    }

    fn decode(&self, _event_type: &str, _stream_id: &str, payload: &[u8]) -> Result<T, Self::Error> {
        serde_json::from_slice(payload)
    }
}
