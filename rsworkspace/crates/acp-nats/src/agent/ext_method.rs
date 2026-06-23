use super::Bridge;
use crate::error::map_nats_error;
use crate::ext_method_name::ExtMethodName;
use crate::nats::{self, RequestClient, global};
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

    let result = nats::request_with_timeout::<N, ExtRequest, ExtResponse>(
        nats,
        &subject,
        &args,
        bridge.config.operation_timeout(),
    )
    .await
    .map_err(map_nats_error);

    bridge
        .metrics
        .record_request("ext_method", bridge.clock.elapsed(start).as_secs_f64(), result.is_ok());

    result
}

#[cfg(test)]
mod tests;
