use agent_client_protocol::v1::schema::{PromptRequest, PromptResponse};
use agent_client_protocol::{Error, ErrorCode};
use tracing::instrument;
use trogon_nats::jetstream::{JetStreamGetStream, JetStreamPublisher, JsRequestMessage};
use trogon_semconv::span::ACP_SESSION_PROMPT;

use crate::agent::Bridge;
use crate::agent::js_request::{JsRequestError, publish_and_await};
use crate::nats::parsing::SessionAgentMethod;
use crate::nats::{FlushClient, PublishClient, RequestClient, commands};
use crate::req_id::ReqId;
use crate::session_id::AcpSessionId;

#[instrument(
    name = ACP_SESSION_PROMPT,
    skip(bridge, args),
    fields(session_id = %args.session_id)
)]
pub async fn handle<N, C, J>(
    bridge: &Bridge<N, C, J>,
    args: PromptRequest,
) -> agent_client_protocol::Result<PromptResponse>
where
    N: RequestClient + PublishClient + FlushClient,
    C: trogon_std::time::GetElapsed,
    J: JetStreamPublisher + JetStreamGetStream,
    trogon_nats::jetstream::JsMessageOf<J>: JsRequestMessage,
{
    let start = bridge.clock.now();

    let session_id = AcpSessionId::try_from(&args.session_id).map_err(|_| {
        bridge.metrics.record_error("prompt", "invalid_session_id");
        Error::new(ErrorCode::INVALID_PARAMS, "Invalid session ID")
    })?;

    let req_id = ReqId::new();
    let prefix = bridge.config.acp_prefix_ref();

    let result = handle_js(bridge, bridge.js(), &args, &session_id, prefix, &req_id).await;

    bridge
        .metrics
        .record_request("prompt", bridge.clock.elapsed(start).as_secs_f64(), result.is_ok());

    result
}

async fn handle_js<N, C, J>(
    bridge: &Bridge<N, C, J>,
    js: &J,
    args: &PromptRequest,
    session_id: &AcpSessionId,
    prefix: &crate::acp_prefix::AcpPrefix,
    req_id: &ReqId,
) -> agent_client_protocol::Result<PromptResponse>
where
    C: trogon_std::time::GetElapsed,
    J: JetStreamPublisher + JetStreamGetStream,
    trogon_nats::jetstream::JsMessageOf<J>: JsRequestMessage,
{
    // Mid-prompt `session/update` progress is not consumed here: it is a client-directed
    // notification and reaches the local client through the client-op proxy
    // (`...client.session.update`), which covers every operation rather than just prompts.
    publish_and_await(
        js,
        commands::PromptSubject::new(prefix, session_id),
        SessionAgentMethod::Prompt.protocol_method(),
        args,
        prefix,
        session_id,
        req_id,
        bridge.config.prompt_timeout(),
    )
    .await
    .map_err(|e| {
        match &e {
            JsRequestError::BadPayload => bridge.metrics.record_error("prompt", "bad_response_payload"),
            JsRequestError::TimedOut { .. } => bridge.metrics.record_error("prompt", "prompt_timeout"),
            JsRequestError::Rejected(_) => {}
        }
        e.into_error("prompt timed out waiting for runner")
    })
}

#[cfg(test)]
mod tests;
