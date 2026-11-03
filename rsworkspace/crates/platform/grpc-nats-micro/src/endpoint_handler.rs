use crate::{content_type::ContentType, service_fault::ServiceFault};
use std::{future::Future, pin::Pin};

/// Decodes a request payload and produces a reply payload for one endpoint.
///
/// The handler receives the request bytes already isolated from NATS
/// transport concerns and returns the success reply body pre-encoded in
/// `content_type`, or a [`ServiceFault`] to report on the micro error
/// channel.
/// Pre-encoding the success body here (rather than a typed message) keeps
/// this trait's signature independent of any one request/response message
/// pair, so one registration loop can dispatch to endpoints with unrelated
/// message types. The trait is boxed-future based (rather than `impl
/// Future`) so `Box<dyn EndpointHandler>` values with different concrete
/// request/response types can share one `Vec` in [`crate::serve`].
pub trait EndpointHandler: Send + Sync {
    fn handle<'a>(
        &'a self,
        request_bytes: &'a [u8],
        content_type: ContentType,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<u8>, ServiceFault>> + Send + 'a>>;
}
