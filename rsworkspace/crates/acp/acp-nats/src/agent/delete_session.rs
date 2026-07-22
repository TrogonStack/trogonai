use super::Bridge;
use crate::nats::parsing::SessionAgentMethod;
use crate::nats::{FlushClient, PublishClient, RequestClient, commands};
use crate::session_id::AcpSessionId;
use agent_client_protocol::schema::v1::{DeleteSessionRequest, DeleteSessionResponse};
use agent_client_protocol::{Error, ErrorCode, Result};
use tracing::{info, instrument};
use trogon_nats::jetstream::{JetStreamGetStream, JetStreamPublisher, JsRequestMessage};
use trogon_semconv::span::ACP_SESSION_DELETE;
use trogon_std::time::GetElapsed;

#[instrument(
    name = ACP_SESSION_DELETE,
    skip(bridge, args),
    fields(session_id = %args.session_id)
)]
pub async fn handle<
    N: RequestClient + PublishClient + FlushClient,
    C: GetElapsed,
    J: JetStreamPublisher + JetStreamGetStream,
>(
    bridge: &Bridge<N, C, J>,
    args: DeleteSessionRequest,
) -> Result<DeleteSessionResponse>
where
    trogon_nats::jetstream::JsMessageOf<J>: JsRequestMessage,
{
    let start = bridge.clock.now();

    info!(session_id = %args.session_id, "Delete session request");

    let session_id = AcpSessionId::try_from(&args.session_id).map_err(|e| {
        bridge.metrics.record_error("session_validate", "invalid_session_id");
        Error::new(ErrorCode::InvalidParams.into(), format!("Invalid session ID: {}", e))
    })?;
    let prefix = bridge.config.acp_prefix_ref();
    let subject = commands::DeleteSubject::new(prefix, &session_id);

    let result = bridge
        .session_request::<DeleteSessionRequest, DeleteSessionResponse>(
            &subject,
            SessionAgentMethod::Delete.wire_method(),
            &args,
            &session_id,
        )
        .await;

    bridge.metrics.record_request(
        "delete_session",
        bridge.clock.elapsed(start).as_secs_f64(),
        result.is_ok(),
    );

    result
}

#[cfg(test)]
mod tests;
