use agent_client_protocol::{Error, ErrorCode};
use async_nats::jetstream::AckKind;
use async_nats::jetstream::consumer::{DeliverPolicy, pull};
use async_nats::subject::ToSubject;
use futures::StreamExt;
use jsonrpc_nats::RequestId;
use serde::de::DeserializeOwned;
use std::fmt;
use std::time::{Duration, SystemTime};
use tokio::time::{Instant, sleep, timeout_at};
use tracing::warn;
use trogon_nats::jetstream::{
    JetStreamConsumer, JetStreamCreateConsumer, JetStreamGetStream, JetStreamPublisher, JsAck as _, JsAckWith as _,
    JsMessageOf, JsMessageRef as _, JsRequestMessage,
};

use crate::acp_prefix::AcpPrefix;
use crate::constants::{RESUME_BACKOFF_MAX, RESUME_BACKOFF_MIN, RESUME_REWIND, SESSION_ID_HEADER};
use crate::jetstream::{consumers, streams};
use crate::req_id::ReqId;
use crate::session_id::AcpSessionId;
use crate::wire::{decode_response, encode_request, req_id_from_request_headers};

type StreamOf<J> = <J as JetStreamGetStream>::Stream;
type ConsumerOf<J> = <StreamOf<J> as JetStreamCreateConsumer>::Consumer;
type MessagesOf<J> = <ConsumerOf<J> as JetStreamConsumer>::Messages;

/// Why the response consumer could not deliver.
#[derive(thiserror::Error)]
pub(crate) enum ResponseConsumerError<J: JetStreamGetStream> {
    #[error("get responses stream: {0}")]
    GetStream(J::Error),
    #[error("create response consumer: {0}")]
    Create(<StreamOf<J> as JetStreamCreateConsumer>::Error),
    #[error("response messages: {0}")]
    Messages(<ConsumerOf<J> as JetStreamConsumer>::StreamError),
    #[error("response consumer: {0}")]
    Delivery(<ConsumerOf<J> as JetStreamConsumer>::MessagesError),
    #[error("response stream closed unexpectedly")]
    Closed,
}

impl<J: JetStreamGetStream> fmt::Debug for ResponseConsumerError<J> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// Why a request ended without the runner's answer.
pub(crate) enum JsRequestError<J: JetStreamGetStream> {
    /// The runner answered with an error, or the request could not be sent.
    Rejected(Error),
    /// The correlated response arrived but its body could not be decoded.
    BadPayload,
    /// The deadline passed. Carries the last consumer error, if any, because
    /// a timeout after consumer errors usually means the network, not the runner.
    TimedOut {
        last_consumer_error: Option<ResponseConsumerError<J>>,
    },
}

impl<J: JetStreamGetStream> JsRequestError<J> {
    pub(crate) fn into_error(self, timeout_message: &str) -> Error {
        match self {
            Self::Rejected(error) => error,
            Self::BadPayload => Error::new(ErrorCode::INTERNAL_ERROR, "bad response payload"),
            Self::TimedOut {
                last_consumer_error: None,
            } => Error::new(ErrorCode::INTERNAL_ERROR, timeout_message),
            Self::TimedOut {
                last_consumer_error: Some(last),
            } => Error::new(
                ErrorCode::INTERNAL_ERROR,
                format!("{timeout_message} (last response consumer error: {last})"),
            ),
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn js_request<J, Req, Res>(
    js: &J,
    subject: &str,
    method: &str,
    request: &Req,
    prefix: &AcpPrefix,
    session_id: &AcpSessionId,
    req_id: &ReqId,
    operation_timeout: Duration,
) -> agent_client_protocol::Result<Res>
where
    J: JetStreamPublisher + JetStreamGetStream,
    JsMessageOf<J>: JsRequestMessage,
    Req: serde::Serialize,
    Res: DeserializeOwned,
{
    publish_and_await(
        js,
        subject.to_string(),
        method,
        request,
        prefix,
        session_id,
        req_id,
        operation_timeout,
    )
    .await
    .map_err(|e| e.into_error("request timed out waiting for runner"))
}

/// Publishes a request on the commands stream and waits for its correlated
/// response on the session's responses stream.
///
/// A failing response consumer is recreated until `operation_timeout`, so a
/// network stall shorter than the timeout does not fail the request.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn publish_and_await<J, Req, Res>(
    js: &J,
    subject: impl ToSubject + Send,
    method: &str,
    request: &Req,
    prefix: &AcpPrefix,
    session_id: &AcpSessionId,
    req_id: &ReqId,
    operation_timeout: Duration,
) -> Result<Res, JsRequestError<J>>
where
    J: JetStreamPublisher + JetStreamGetStream,
    JsMessageOf<J>: JsRequestMessage,
    Req: serde::Serialize,
    Res: DeserializeOwned,
{
    let rejected = |message: String| JsRequestError::Rejected(Error::new(ErrorCode::INTERNAL_ERROR, message));

    // Create the consumer BEFORE publishing — same principle as subscribe-before-publish.
    // It is session-scoped and delivers only new messages, so creating it first is what
    // guarantees the response is seen even if the runner answers before we start consuming.
    let mut responses = ResponseConsumer::open(js, prefix, session_id)
        .await
        .map_err(|e| rejected(e.to_string()))?;

    let encoded = encode_request(method, RequestId::String(req_id.as_str().to_string()), request)
        .map_err(|e| rejected(format!("encode request: {e}")))?;

    let mut headers = encoded.headers;
    headers.insert(SESSION_ID_HEADER, session_id.as_str());

    js.publish_with_headers(subject, headers, encoded.body)
        .await
        .map_err(|e| rejected(format!("js publish: {e}")))?
        .await
        .map_err(|e| rejected(format!("js ack: {e}")))?;

    match timeout_at(Instant::now() + operation_timeout, responses.await_response(req_id)).await {
        Ok(result) => result,
        Err(_elapsed) => Err(JsRequestError::TimedOut {
            last_consumer_error: responses.last_failure.take(),
        }),
    }
}

/// A session-scoped response consumer that recreates itself after failures.
struct ResponseConsumer<'a, J: JetStreamGetStream> {
    js: &'a J,
    stream_name: String,
    /// Config for recreated consumers: the original filter, rewound in time.
    resume_config: pull::Config,
    messages: MessagesOf<J>,
    last_failure: Option<ResponseConsumerError<J>>,
}

impl<'a, J> ResponseConsumer<'a, J>
where
    J: JetStreamGetStream,
    JsMessageOf<J>: JsRequestMessage,
{
    async fn open(js: &'a J, prefix: &AcpPrefix, session_id: &AcpSessionId) -> Result<Self, ResponseConsumerError<J>> {
        let stream_name = streams::responses_stream_name(prefix);
        let config = consumers::response_consumer(prefix, session_id);
        let rewind_to = SystemTime::now()
            .checked_sub(RESUME_REWIND)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let resume_config = pull::Config {
            deliver_policy: DeliverPolicy::ByStartTime {
                start_time: rewind_to.into(),
            },
            ..config.clone()
        };
        let messages = subscribe(js, &stream_name, config).await?;
        Ok(Self {
            js,
            stream_name,
            resume_config,
            messages,
            last_failure: None,
        })
    }

    /// Returns the first response addressed to `req_id`, acking and skipping
    /// replies to other requests on the session. Never times out on its own;
    /// the caller bounds it.
    ///
    /// Returning on the first match is what keeps a reread response from being
    /// delivered twice after the consumer was recreated.
    async fn await_response<Res: DeserializeOwned>(&mut self, req_id: &ReqId) -> Result<Res, JsRequestError<J>> {
        loop {
            let failure = match self.messages.next().await {
                Some(Ok(js_msg)) => {
                    let message = js_msg.message();
                    let response_headers = message.headers.clone().unwrap_or_default();

                    if req_id_from_request_headers(&response_headers).as_ref() != Some(req_id) {
                        let _ = js_msg.ack().await;
                        continue;
                    }

                    return match decode_response::<Res>(&response_headers, message.payload.as_ref()) {
                        Ok(answer) => {
                            let _ = js_msg.ack().await;
                            answer.map_err(JsRequestError::Rejected)
                        }
                        Err(_) => {
                            let _ = js_msg.ack_with(AckKind::Term).await;
                            Err(JsRequestError::BadPayload)
                        }
                    };
                }
                Some(Err(e)) => ResponseConsumerError::Delivery(e),
                None => ResponseConsumerError::Closed,
            };
            warn!(error = %failure, "Response consumer failed; recreating it");
            self.last_failure = Some(failure);
            self.resume().await;
        }
    }

    /// Recreates the consumer, backing off between attempts, until it succeeds.
    async fn resume(&mut self) {
        let mut backoff = RESUME_BACKOFF_MIN;
        loop {
            sleep(backoff).await;
            match subscribe(self.js, &self.stream_name, self.resume_config.clone()).await {
                Ok(messages) => {
                    self.messages = messages;
                    return;
                }
                Err(failure) => {
                    warn!(error = %failure, "Recreating the response consumer failed; retrying");
                    self.last_failure = Some(failure);
                }
            }
            backoff = (backoff * 2).min(RESUME_BACKOFF_MAX);
        }
    }
}

async fn subscribe<J: JetStreamGetStream>(
    js: &J,
    stream_name: &str,
    config: pull::Config,
) -> Result<MessagesOf<J>, ResponseConsumerError<J>> {
    let stream = js
        .get_stream(stream_name)
        .await
        .map_err(ResponseConsumerError::GetStream)?;
    let consumer = stream
        .create_consumer(config)
        .await
        .map_err(ResponseConsumerError::Create)?;
    consumer.messages().await.map_err(ResponseConsumerError::Messages)
}

#[cfg(test)]
mod tests;
