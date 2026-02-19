use super::Bridge;
use crate::nats::{self, FlushClient, PublishClient, RequestClient, SubscribeClient, agent};
use agent_client_protocol::{AuthenticateRequest, AuthenticateResponse, Error, Result};
use std::time::Instant;
use tracing::{info, instrument};

#[instrument(
    name = "acp.authenticate",
    skip(bridge, args),
    fields(method_id = %args.method_id)
)]
pub async fn handle<N: SubscribeClient + RequestClient + PublishClient + FlushClient>(
    bridge: &Bridge<N>,
    args: AuthenticateRequest,
) -> Result<AuthenticateResponse> {
    let start = Instant::now();

    info!(method_id = %args.method_id, "Authenticate request");

    let nats = bridge.require_nats()?;

    let result = nats::request::<N, AuthenticateRequest, AuthenticateResponse>(
        nats,
        &agent::authenticate(&bridge.acp_prefix),
        &args,
    )
    .await
    .map_err(|e| Error::new(-32603, e.to_string()));

    bridge.metrics.record_request(
        "authenticate",
        start.elapsed().as_secs_f64(),
        result.is_ok(),
    );

    result
}
