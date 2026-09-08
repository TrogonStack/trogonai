use std::fmt::Debug;

use futures_util::{Stream, StreamExt};
use tracing::{info, warn};
use twilight_gateway::Message;

use super::gateway::GatewayBridge;

pub async fn run<P, S, Incoming, E>(
    publisher: trogon_nats::jetstream::ClaimCheckPublisher<P, S>,
    config: super::config::DiscordConfig,
    mut incoming: Incoming,
) where
    P: trogon_nats::jetstream::JetStreamPublisher,
    S: trogon_nats::jetstream::ObjectStorePut,
    Incoming: Stream<Item = Result<Message, E>> + Unpin,
    E: Debug,
{
    info!("mode: gateway");

    let bridge = GatewayBridge::new(publisher, config.subject_prefix.clone(), config.nats_ack_timeout.into());

    info!("starting Discord gateway connection");

    loop {
        let msg = incoming.next().await;
        match msg {
            Some(Ok(Message::Text(text))) => bridge.dispatch(&text).await,
            Some(Ok(Message::Close(_))) => {
                info!("gateway connection closed");
                break;
            }
            Some(Err(source)) => {
                warn!(?source, "error receiving gateway message");
                continue;
            }
            None => break,
        }
    }
}

#[cfg(test)]
mod tests;
