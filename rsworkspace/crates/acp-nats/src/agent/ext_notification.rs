use super::Bridge;
use crate::nats::{self, agent, FlushClient, PublishClient, RequestClient, SubscribeClient};
use agent_client_protocol::{ExtNotification, Result};
use std::time::Instant;
use tracing::{info, instrument, warn};

#[instrument(
    name = "acp.ext.notification",
    skip(bridge, args),
    fields(method = %args.method)
)]
pub async fn handle<N: SubscribeClient + RequestClient + PublishClient + FlushClient>(bridge: &Bridge<N>, args: ExtNotification) -> Result<()> {
    let start = Instant::now();

    info!(method = %args.method, "Extension notification");

    if let Some(nats) = &bridge.nats {
        let subject = agent::ext(&bridge.acp_prefix, &args.method);

        if let Err(e) = nats::publish(nats, &subject, &args, nats::PublishOptions::simple()).await {
            warn!(error = %e, method = %args.method, "Failed to publish extension notification");
        }
    }

    bridge
        .metrics
        .record_request("ext_notification", start.elapsed().as_secs_f64(), true);

    Ok(())
}
