use std::fmt;

use a2a_nats::{NatsConfig, ingress_invalid_request_response_bytes, resolve_gateway_ingress_subject};
use async_nats::HeaderMap;
use futures::stream::StreamExt;
use tokio_util::sync::CancellationToken;
use tracing::{Instrument, debug, info, warn};

use crate::config::{Args, Config, ConfigError, config_from_args};

#[derive(Debug)]
pub enum RuntimeError {
    Config(ConfigError),
    NatsConnect(trogon_nats::ConnectError),
    Subscribe(String),
}

impl fmt::Display for RuntimeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config(error) => write!(f, "{error}"),
            Self::NatsConnect(error) => write!(f, "NATS connection failed: {error}"),
            Self::Subscribe(msg) => write!(f, "gateway subscribe failed: {msg}"),
        }
    }
}

impl std::error::Error for RuntimeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Config(error) => Some(error),
            Self::NatsConnect(error) => Some(error),
            Self::Subscribe(_) => None,
        }
    }
}

impl From<ConfigError> for RuntimeError {
    fn from(error: ConfigError) -> Self {
        Self::Config(error)
    }
}

pub async fn run_with_args<E: trogon_std::env::ReadEnv>(args: Args, env: &E) -> Result<(), RuntimeError> {
    let (config, nats_config) = config_from_args(args, env)?;

    run_with_config(config, nats_config, env).await
}

pub async fn run_with_config<E: trogon_std::env::ReadEnv>(
    config: Config,
    nats_config: NatsConfig,
    env: &E,
) -> Result<(), RuntimeError> {
    let connect_timeout = a2a_nats::nats_connect_timeout(env);
    let client = trogon_nats::connect(&nats_config, connect_timeout)
        .await
        .map_err(RuntimeError::NatsConnect)?;

    let gateway_subject_string = config.gateway_subscribe_subject();
    let gateway_subject = async_nats::Subject::from(gateway_subject_string.as_str());

    let mut ingress = match &config.queue_group {
        Some(q) => client
            .queue_subscribe(gateway_subject, q.clone())
            .await
            .map_err(|e| RuntimeError::Subscribe(e.to_string()))?,
        None => client
            .subscribe(gateway_subject)
            .await
            .map_err(|e| RuntimeError::Subscribe(e.to_string()))?,
    };

    info!(
        prefix = %config.a2a_prefix,
        gateway_subject = %gateway_subject_string,
        queue_group = config.queue_group.as_deref().unwrap_or("(none — ephemeral subscriber)"),
        servers = ?config.nats_servers,
        "gateway subscribed on ingress wildcard; routing to mapped agent RPC subjects"
    );

    let shutdown = CancellationToken::new();
    let shutdown_for_task = shutdown.clone();

    tokio::spawn(async move {
        trogon_std::signal::shutdown_signal().await;
        shutdown_for_task.cancel();
    });

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => {
                info!(prefix = %config.a2a_prefix, "gateway shutdown signal received");
                break;
            }
            incoming = ingress.next() => {
                match incoming {
                    Some(msg) => {
                        dispatch_gateway_ingress(&client, &config, msg).await;
                    }
                    None => {
                        warn!("gateway ingress NATS subscription closed");
                        break;
                    }
                }
            }
        }
    }

    drop(ingress);

    info!(prefix = %config.a2a_prefix, "A2A gateway shutdown complete");
    Ok(())
}

async fn dispatch_gateway_ingress(client: &async_nats::Client, config: &Config, msg: async_nats::Message) {
    let ingress_subject = msg.subject.as_str();
    let reply_present = msg.reply.is_some();

    let span = tracing::info_span!(
        "gateway.ingress.dispatch",
        gateway_ingress.subject = %ingress_subject,
        ingress.reply_present = reply_present,
        caller_id = tracing::field::Empty, // future: JWT-derived identity from auth-callout
        agent_subject = tracing::field::Empty,
        routing_outcome = tracing::field::Empty,
    );

    async {
        debug!(
            ingress.subject = %msg.subject,
            ingress.reply_present = reply_present,
            payload_len = msg.payload.len(),
            "gateway ingress envelope received",
        );

        let Some(reply) = msg.reply else {
            tracing::Span::current().record("routing_outcome", "ignored_no_reply");
            warn!(
                ingress.subject = %msg.subject,
                ingress.reply_present = false,
                routing_outcome = "ignored_no_reply",
                "gateway ingress without reply inbox; ignoring",
            );
            return;
        };

        match resolve_gateway_ingress_subject(msg.subject.as_str(), &config.a2a_prefix) {
            Ok(agent_subject) => {
                tracing::Span::current().record("agent_subject", tracing::field::display(&agent_subject));
                debug!(
                    ingress.subject = %msg.subject,
                    agent_subject = %agent_subject,
                    ingress.reply_present = true,
                    reply = %reply,
                    "gateway forwarding to agent subject",
                );
                let headers = msg.headers.unwrap_or_default();
                match client
                    .publish_with_reply_and_headers(agent_subject.clone(), reply.clone(), headers, msg.payload)
                    .await
                {
                    Ok(()) => {
                        tracing::Span::current().record("routing_outcome", "forwarded");
                    }
                    Err(error) => {
                        tracing::Span::current().record("routing_outcome", "forward_failed");
                        warn!(
                            ingress.subject = %msg.subject,
                            agent_subject = %agent_subject,
                            ingress.reply_present = true,
                            routing_outcome = "forward_failed",
                            error = %error,
                            "gateway failed to publish forward to agent subject",
                        );
                    }
                }
            }
            Err(reason) => {
                tracing::Span::current().record("routing_outcome", "ingress_error");
                warn!(
                    ingress.subject = %msg.subject,
                    ingress.reply_present = true,
                    routing_outcome = "ingress_error",
                    reason = %reason,
                    reply = %reply,
                    "gateway ingress subject routing failed",
                );
                let body = match ingress_invalid_request_response_bytes(&msg.payload, reason.to_string()) {
                    Ok(b) => b,
                    Err(error) => {
                        warn!(
                            ingress.subject = %msg.subject,
                            ingress.reply_present = true,
                            routing_outcome = "ingress_error",
                            error = %error,
                            "failed to serialize JSON-RPC ingress error response",
                        );
                        return;
                    }
                };
                let headers = HeaderMap::new();
                if let Err(error) = client.publish_with_headers(reply.clone(), headers, body).await {
                    warn!(
                        ingress.subject = %msg.subject,
                        ingress.reply_present = true,
                        routing_outcome = "ingress_error",
                        reply = %reply,
                        error = %error,
                        "gateway failed to publish ingress error reply",
                    );
                }
            }
        }
    }
    .instrument(span)
    .await
}

#[cfg(test)]
mod gateway_dispatch_tests {
    use trogon_std::env::InMemoryEnv;

    use a2a_nats::resolve_gateway_ingress_subject;

    use super::*;
    use crate::Args;

    fn test_config(prefix: &str) -> Config {
        let env = InMemoryEnv::new();
        let args = Args {
            nats_url: "localhost:4222".into(),
            prefix: prefix.into(),
            queue_group: None,
        };
        config_from_args(args, &env).unwrap().0
    }

    #[test]
    fn dispatch_builds_publish_args_from_valid_ingress_subject() {
        let cfg = test_config("a2a");
        assert_eq!(
            resolve_gateway_ingress_subject("a2a.gateway.planner.message.send", &cfg.a2a_prefix).unwrap(),
            "a2a.agent.planner.message.send"
        );
    }
}
