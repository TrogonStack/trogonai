mod canonical;
mod reconstruct;

use async_nats::header::HeaderMap;
use bytes::Bytes;

pub use canonical::{decode, decode_value, encode, encode_value};
pub use reconstruct::{from_json_value, to_json_value};

/// NATS wire representation produced by [`encode`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Encoded {
    pub headers: HeaderMap,
    pub body: Bytes,
}
