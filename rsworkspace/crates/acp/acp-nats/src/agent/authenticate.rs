use super::Bridge;
use super::rpc_call::jsonrpc_call;
use crate::nats::{RequestClient, global};
use agent_client_protocol::Result;
use agent_client_protocol::schema::v1::{AuthenticateRequest, AuthenticateResponse};
use tracing::{info, instrument};
use trogon_semconv::span::ACP_AUTHENTICATE;
use trogon_std::time::GetElapsed;

#[instrument(
    name = ACP_AUTHENTICATE,
    skip(bridge, args),
    fields(method_id = %args.method_id)
)]
pub async fn handle<N: RequestClient, C: GetElapsed, J>(
    bridge: &Bridge<N, C, J>,
    args: AuthenticateRequest,
) -> Result<AuthenticateResponse> {
    let start = bridge.clock.now();

    info!(method_id = %args.method_id, "Authenticate request");

    let result = jsonrpc_call(
        bridge.nats(),
        &global::AuthenticateSubject::new(bridge.config.acp_prefix_ref()),
        "authenticate",
        &args,
        bridge.config.operation_timeout,
    )
    .await;

    bridge.metrics.record_request(
        "authenticate",
        bridge.clock.elapsed(start).as_secs_f64(),
        result.is_ok(),
    );

    result
}

#[cfg(test)]
mod tests;
