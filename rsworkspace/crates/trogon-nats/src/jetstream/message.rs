use std::error::Error;
use std::future::Future;
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsSignal {
    Ack,
    DoubleAck,
    Nak,
    NakWithDelay(Duration),
    Progress,
    Term,
}

pub trait JsMessage: Send + 'static {
    type Error: Error + Send + Sync;

    fn payload(&self) -> &bytes::Bytes;
    fn subject(&self) -> &str;
    fn headers(&self) -> Option<&async_nats::HeaderMap>;
    fn reply(&self) -> Option<&str>;

    fn ack(&self) -> impl Future<Output = Result<(), Self::Error>> + Send;
    fn double_ack(&self) -> impl Future<Output = Result<(), Self::Error>> + Send;
    fn nak(&self) -> impl Future<Output = Result<(), Self::Error>> + Send;
    fn nak_with_delay(
        &self,
        delay: Duration,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send;
    fn term(&self) -> impl Future<Output = Result<(), Self::Error>> + Send;
    fn in_progress(&self) -> impl Future<Output = Result<(), Self::Error>> + Send;
}
