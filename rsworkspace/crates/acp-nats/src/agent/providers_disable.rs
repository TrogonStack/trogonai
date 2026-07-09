use super::Bridge;
use super::rpc_call::jsonrpc_call;
use crate::nats::{RequestClient, global};
use agent_client_protocol::Result;
use agent_client_protocol::schema::v1::{DisableProviderRequest, DisableProviderResponse};
use tracing::{info, instrument};
use trogon_semconv::span::ACP_PROVIDERS_DISABLE;
use trogon_std::time::GetElapsed;

#[instrument(name = ACP_PROVIDERS_DISABLE, skip(bridge, args))]
pub async fn handle<N: RequestClient, C: GetElapsed, J>(
    bridge: &Bridge<N, C, J>,
    args: DisableProviderRequest,
) -> Result<DisableProviderResponse> {
    let start = bridge.clock.now();

    info!("Disable provider request");

    let subject = global::ProvidersDisableSubject::new(bridge.config.acp_prefix_ref());

    let result = jsonrpc_call(
        bridge.nats(),
        &subject,
        "providers.disable",
        &args,
        bridge.config.operation_timeout,
    )
    .await;

    bridge.metrics.record_request(
        "disable_provider",
        bridge.clock.elapsed(start).as_secs_f64(),
        result.is_ok(),
    );

    result
}

#[cfg(test)]
mod tests;
