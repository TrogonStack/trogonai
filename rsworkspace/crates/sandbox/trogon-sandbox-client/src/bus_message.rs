use bytes::Bytes;

/// One message received from a [`Bus`](crate::Bus) subscription.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BusMessage {
    pub subject: String,
    pub payload: Bytes,
}
