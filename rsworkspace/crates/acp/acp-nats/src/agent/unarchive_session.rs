use super::Bridge;
use crate::nats::parsing::SessionAgentMethod;
use crate::nats::{FlushClient, PublishClient, RequestClient, commands};
use crate::session_id::AcpSessionId;
use agent_client_protocol::v1::schema::{UnarchiveSessionRequest, UnarchiveSessionResponse};
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
    args: UnarchiveSessionRequest,
) -> Result<UnarchiveSessionResponse>
where
    trogon_nats::jetstream::JsMessageOf<J>: JsRequestMessage,
{
    let start = bridge.clock.now();

    info!(session_id = %args.session_id, "Unarchive session request");

    let session_id = AcpSessionId::try_from(&args.session_id).map_err(|e| {
        bridge.metrics.record_error("session_validate", "invalid_session_id");
        Error::new(ErrorCode::INVALID_PARAMS, format!("Invalid session ID: {}", e))
    })?;
    let prefix = bridge.config.acp_prefix_ref();
    let subject = commands::UnarchiveSubject::new(prefix, &session_id);

    let result = bridge
        .session_request::<UnarchiveSessionRequest, UnarchiveSessionResponse>(
            &subject,
            SessionAgentMethod::Unarchive.protocol_method(),
            &args,
            &session_id,
        )
        .await;

    bridge.metrics.record_request(
        "unarchive_session",
        bridge.clock.elapsed(start).as_secs_f64(),
        result.is_ok(),
    );

    result
}
