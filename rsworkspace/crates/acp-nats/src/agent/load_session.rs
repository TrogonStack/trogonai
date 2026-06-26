use super::Bridge;
use crate::nats::parsing::SessionAgentMethod;
use crate::nats::{FlushClient, PublishClient, RequestClient, commands};
use crate::session_id::AcpSessionId;
use agent_client_protocol::{Error, ErrorCode, LoadSessionRequest, LoadSessionResponse, Result};
use tracing::{info, instrument};
use trogon_semconv::span::ACP_SESSION_LOAD;
use trogon_nats::jetstream::{JetStreamGetStream, JetStreamPublisher, JsRequestMessage};
use trogon_std::time::GetElapsed;

#[instrument(
    name = ACP_SESSION_LOAD,
    skip(bridge, args),
    fields(session_id = %args.session_id)
)]
pub async fn handle<
    N: RequestClient + PublishClient + FlushClient,
    C: GetElapsed,
    J: JetStreamPublisher + JetStreamGetStream,
>(
    bridge: &Bridge<N, C, J>,
    args: LoadSessionRequest,
) -> Result<LoadSessionResponse>
where
    trogon_nats::jetstream::JsMessageOf<J>: JsRequestMessage,
{
    let start = bridge.clock.now();

    info!(session_id = %args.session_id, "Load session request");

    let session_id = AcpSessionId::try_from(&args.session_id).map_err(|e| {
        bridge.metrics.record_error("session_validate", "invalid_session_id");
        Error::new(ErrorCode::InvalidParams.into(), format!("Invalid session ID: {}", e))
    })?;
    let prefix = bridge.config.acp_prefix_ref();
    let subject = commands::LoadSubject::new(prefix, &session_id);

    let result = bridge
        .session_request::<LoadSessionRequest, LoadSessionResponse>(
            &subject,
            SessionAgentMethod::Load.wire_method(),
            &args,
            &session_id,
        )
        .await;

    if result.is_ok() {
        bridge.schedule_session_ready(args.session_id.clone());
    }

    bridge.metrics.record_request(
        "load_session",
        bridge.clock.elapsed(start).as_secs_f64(),
        result.is_ok(),
    );

    result
}

#[cfg(test)]
mod tests;
