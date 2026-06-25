use super::Bridge;
use super::rpc_call::jsonrpc_call;
use crate::ext_method_name::ExtMethodName;
use crate::nats::{RequestClient, global};
use agent_client_protocol::{Error, ErrorCode, ExtRequest, ExtResponse, Result};
use tracing::{info, instrument};
use trogon_std::time::GetElapsed;

#[instrument(
    name = "acp.ext",
    skip(bridge, args),
    fields(method = %args.method)
)]
pub async fn handle<N: RequestClient, C: GetElapsed, J>(
    bridge: &Bridge<N, C, J>,
    args: ExtRequest,
) -> Result<ExtResponse> {
    let start = bridge.clock.now();

    info!(method = %args.method, "Extension method request");

    let method_name = ExtMethodName::new(&args.method).map_err(|e| {
        bridge
            .metrics
            .record_request("ext_method", bridge.clock.elapsed(start).as_secs_f64(), false);
        bridge.metrics.record_error("ext_method", "invalid_method_name");
        Error::new(ErrorCode::InvalidParams.into(), format!("Invalid method name: {}", e))
    })?;

    let nats = bridge.nats();
    let subject = global::ExtSubject::new(bridge.config.acp_prefix_ref(), &method_name);
    let wire_method = format!("ext.{}", method_name.as_str());

    let result = jsonrpc_call(nats, &subject, &wire_method, &args, bridge.config.operation_timeout()).await;

    bridge
        .metrics
        .record_request("ext_method", bridge.clock.elapsed(start).as_secs_f64(), result.is_ok());

    result
}

#[cfg(test)]
mod tests;
