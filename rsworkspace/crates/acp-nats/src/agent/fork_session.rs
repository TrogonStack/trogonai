use super::Bridge;
use crate::nats::parsing::SessionAgentMethod;
use crate::nats::{FlushClient, PublishClient, RequestClient, commands};
use crate::session_id::AcpSessionId;
use agent_client_protocol::schema::v1::{ForkSessionRequest, ForkSessionResponse};
use agent_client_protocol::{Error, ErrorCode, Result};
use tracing::{Span, info, instrument};
use trogon_nats::jetstream::{JetStreamGetStream, JetStreamPublisher, JsRequestMessage};
use trogon_semconv::attribute::NEW_SESSION_ID;
use trogon_semconv::span::ACP_SESSION_FORK;
use trogon_std::time::GetElapsed;

#[instrument(
    name = ACP_SESSION_FORK,
    skip(bridge, args),
    fields(session_id = %args.session_id, new_session_id = tracing::field::Empty)
)]
pub async fn handle<
    N: RequestClient + PublishClient + FlushClient,
    C: GetElapsed,
    J: JetStreamPublisher + JetStreamGetStream,
>(
    bridge: &Bridge<N, C, J>,
    args: ForkSessionRequest,
) -> Result<ForkSessionResponse>
where
    trogon_nats::jetstream::JsMessageOf<J>: JsRequestMessage,
{
    let start = bridge.clock.now();

    info!(session_id = %args.session_id, "Fork session request");

    let session_id = AcpSessionId::try_from(&args.session_id).map_err(|e| {
        bridge.metrics.record_error("session_validate", "invalid_session_id");
        Error::new(ErrorCode::InvalidParams.into(), format!("Invalid session ID: {}", e))
    })?;
    let prefix = bridge.config.acp_prefix_ref();
    let subject = commands::ForkSubject::new(prefix, &session_id);

    let result = bridge
        .session_request::<ForkSessionRequest, ForkSessionResponse>(
            &subject,
            SessionAgentMethod::Fork.wire_method(),
            &args,
            &session_id,
        )
        .await;

    if let Ok(ref response) = result {
        Span::current().record(NEW_SESSION_ID, response.session_id.to_string().as_str());
        info!(new_session_id = %response.session_id, "Session forked");

        bridge.schedule_session_ready(response.session_id.clone());
    }

    bridge.metrics.record_request(
        "fork_session",
        bridge.clock.elapsed(start).as_secs_f64(),
        result.is_ok(),
    );

    result
}

#[cfg(test)]
mod tests;
