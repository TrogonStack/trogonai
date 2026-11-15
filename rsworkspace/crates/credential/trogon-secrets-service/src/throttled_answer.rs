use std::future::Future;
use std::pin::Pin;

use grpc_nats_micro::{ContentType, EndpointHandler, ServiceFault};

use super::release_reply::ReleaseReply;

/// Answers a call the release endpoint has no room for: `THROTTLED`, with the
/// delay after which every release in flight has answered.
///
/// It reads neither the subject nor the payload, so a throttled caller learns
/// nothing of whether its call would have been released.
pub(crate) struct ThrottledAnswer;

impl EndpointHandler for ThrottledAnswer {
    fn handle<'a>(
        &'a self,
        _request_bytes: &'a [u8],
        content_type: ContentType,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, ServiceFault>> + Send + 'a>> {
        Box::pin(async move { ReleaseReply::throttled().encode(content_type) })
    }
}

#[cfg(test)]
mod tests;
