//! The HTTP side of the read stream: sends `GET <session>/worker/events/stream` through the client's [`HttpExecute`]
//! and hands the body over a chunk at a time, so the timers of [`run_stream`](crate::run_stream) see the same
//! status, refusal body and frames the upstream `fetch` and its reader do.

use std::sync::Arc;

use crate::adapters::HttpExecute;
use crate::bridge_worker::WorkerClient;
use crate::bridge_worker_stream::StreamRequest;
use crate::bridge_worker_stream_driver::{StreamBody, StreamConnector, StreamFailureError, StreamResponse};

/// Opens the read stream of one worker client with the token the client holds at that moment.
pub struct HttpStreamConnector<H, P> {
    client: Arc<WorkerClient<H, P>>,
}

impl<H, P> HttpStreamConnector<H, P> {
    pub fn new(client: Arc<WorkerClient<H, P>>) -> Self {
        Self { client }
    }
}

/// The body of an answer, read with [`reqwest::Response::chunk`].
pub struct HttpStreamBody {
    response: reqwest::Response,
}

impl StreamBody for HttpStreamBody {
    async fn next_chunk(&mut self) -> Option<Result<Vec<u8>, StreamFailureError>> {
        match self.response.chunk().await {
            Ok(Some(chunk)) => Some(Ok(chunk.to_vec())),
            Ok(None) => None,
            Err(error) => Some(Err(StreamFailureError(error.to_string()))),
        }
    }
}

impl<H: HttpExecute + Send + Sync, P: Send + Sync> StreamConnector for HttpStreamConnector<H, P> {
    type Body = HttpStreamBody;

    async fn open(&self, request: &StreamRequest) -> Result<StreamResponse<HttpStreamBody>, StreamFailureError> {
        let request = self
            .client
            .stream_request(request)
            .ok_or_else(|| StreamFailureError("the request could not be built".to_owned()))?;
        let response = self
            .client
            .http()
            .execute(request)
            .await
            .map_err(|error| StreamFailureError(error.to_string()))?;
        Ok(StreamResponse {
            status: response.status().as_u16(),
            body: HttpStreamBody { response },
        })
    }
}
