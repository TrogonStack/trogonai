use super::Bridge;
use crate::nats::parsing::SessionAgentMethod;
use crate::nats::{FlushClient, PublishClient, RequestClient, commands};
use crate::session_id::AcpSessionId;
use agent_client_protocol::v1::schema::{SessionStatusRequest, SessionStatusResponse};
use agent_client_protocol::{Error, ErrorCode, Result};
use tracing::info;
use trogon_nats::jetstream::{JetStreamGetStream, JetStreamPublisher, JsRequestMessage};
use trogon_std::time::GetElapsed;

pub async fn handle<
    N: RequestClient + PublishClient + FlushClient,
    C: GetElapsed,
    J: JetStreamPublisher + JetStreamGetStream,
>(
    bridge: &Bridge<N, C, J>,
    args: SessionStatusRequest,
) -> Result<SessionStatusResponse>
where
    trogon_nats::jetstream::JsMessageOf<J>: JsRequestMessage,
{
    let start = bridge.clock.now();

    info!(session_id = %args.session_id, "Session status request");

    let session_id = AcpSessionId::try_from(&args.session_id).map_err(|e| {
        bridge.metrics.record_error("session_validate", "invalid_session_id");
        Error::new(ErrorCode::INVALID_PARAMS, format!("Invalid session ID: {}", e))
    })?;
    let prefix = bridge.config.acp_prefix_ref();
    let subject = commands::StatusSubject::new(prefix, &session_id);

    let result = bridge
        .session_request::<SessionStatusRequest, SessionStatusResponse>(
            &subject,
            SessionAgentMethod::Status.protocol_method(),
            &args,
            &session_id,
        )
        .await;

    bridge.metrics.record_request(
        "session_status",
        bridge.clock.elapsed(start).as_secs_f64(),
        result.is_ok(),
    );

    result
}
