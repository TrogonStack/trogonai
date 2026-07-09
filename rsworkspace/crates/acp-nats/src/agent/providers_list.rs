use super::Bridge;
use super::rpc_call::jsonrpc_call;
use crate::nats::{GlobalAgentMethod, RequestClient, global};
use agent_client_protocol::Result;
use agent_client_protocol::schema::v1::{ListProvidersRequest, ListProvidersResponse};
use tracing::{info, instrument};
use trogon_semconv::span::ACP_PROVIDERS_LIST;
use trogon_std::time::GetElapsed;

#[instrument(name = ACP_PROVIDERS_LIST, skip(bridge, args))]
pub async fn handle<N: RequestClient, C: GetElapsed, J>(
    bridge: &Bridge<N, C, J>,
    args: ListProvidersRequest,
) -> Result<ListProvidersResponse> {
    let start = bridge.clock.now();

    info!("List providers request");

    let subject = global::ProvidersListSubject::new(bridge.config.acp_prefix_ref());
    let method = GlobalAgentMethod::ProvidersList.wire_method();

    let result = jsonrpc_call(bridge.nats(), &subject, &method, &args, bridge.config.operation_timeout).await;

    bridge.metrics.record_request(
        "list_providers",
        bridge.clock.elapsed(start).as_secs_f64(),
        result.is_ok(),
    );

    result
}

#[cfg(test)]
mod tests;
