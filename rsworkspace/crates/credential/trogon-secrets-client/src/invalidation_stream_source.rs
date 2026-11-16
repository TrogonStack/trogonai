use std::future::Future;

use async_nats::jetstream::{self, context::GetStreamError};

use super::InvalidationStream;

/// Looks up the stream release invalidations are read from.
pub trait InvalidationStreamSource: Send + Sync + 'static {
    type Stream: InvalidationStream;

    fn get_stream(&self, name: &str) -> impl Future<Output = Result<Self::Stream, GetStreamError>> + Send;
}

impl InvalidationStreamSource for jetstream::Context {
    type Stream = jetstream::stream::Stream;

    async fn get_stream(&self, name: &str) -> Result<Self::Stream, GetStreamError> {
        jetstream::Context::get_stream(self, name).await
    }
}
