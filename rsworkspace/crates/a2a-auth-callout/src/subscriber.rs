use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt as _;
use tracing::{error, info, warn};

use crate::denial_claims::CalloutIssuer;
use crate::dispatcher::AuthDispatcher;
use crate::error::AuthCalloutError;
use crate::jwt::SigningKey;
use crate::wire::AuthCalloutWireCodec;

/// NATS subject the server uses for auth callout requests.
const AUTH_CALLOUT_SUBJECT: &str = "$SYS.REQ.USER.AUTH";

const DEFAULT_DENIAL_TTL: Duration = Duration::from_secs(60);

/// Signing and issuer material for NATS authorization-response denial JWTs.
pub struct DenialPublisherConfig {
    pub signing_key: SigningKey,
    pub issuer: CalloutIssuer,
    pub denial_ttl: Duration,
}

impl DenialPublisherConfig {
    pub fn new(signing_key: SigningKey, issuer: CalloutIssuer) -> Self {
        Self {
            signing_key,
            issuer,
            denial_ttl: DEFAULT_DENIAL_TTL,
        }
    }
}

/// Subscriber loop that listens on `$SYS.REQ.USER.AUTH` and delegates each
/// request to the injected `AuthDispatcher`.
pub struct Subscriber<D> {
    client: async_nats::Client,
    dispatcher: Arc<D>,
    wire: Arc<AuthCalloutWireCodec>,
}

impl<D: AuthDispatcher> Subscriber<D> {
    pub fn new(client: async_nats::Client, dispatcher: D, wire: AuthCalloutWireCodec) -> Self {
        Self {
            client,
            dispatcher: Arc::new(dispatcher),
            wire: Arc::new(wire),
        }
    }

    pub async fn run(self) -> Result<(), AuthCalloutError> {
        // Use a queue subscription so replicas form an HA group on a single
        // worker queue — plain `subscribe` would deliver each
        // $SYS.REQ.USER.AUTH request to every replica and have all of them
        // publish to the same reply subject, racing each other for one
        // server request and producing unpredictable client connect outcomes.
        // The queue name is operator-overridable but defaults to a constant
        // so two callout deployments at the same scope share it.
        let queue = std::env::var("AUTH_CALLOUT_QUEUE_GROUP").unwrap_or_else(|_| "a2a-auth-callout".to_string());
        let mut sub = self
            .client
            .queue_subscribe(AUTH_CALLOUT_SUBJECT, queue)
            .await
            .map_err(|e| AuthCalloutError::Subscribe(e.to_string()))?;

        info!(subject = AUTH_CALLOUT_SUBJECT, "auth callout subscriber started");

        if let Ok(path) = std::env::var("AUTH_CALLOUT_READY_FILE") {
            if let Some(parent) = std::path::Path::new(&path).parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            if std::fs::write(&path, "ready\n").is_err() {
                warn!(path = %path, "failed to write auth-callout readiness marker");
            }
        }

        while let Some(msg) = sub.next().await {
            let reply = match msg.reply.clone() {
                Some(r) => r,
                None => {
                    warn!("auth callout request without reply subject; dropping");
                    continue;
                }
            };

            let request = match self.wire.decode_request(msg.payload.to_vec(), msg.headers.as_ref()) {
                Ok(r) => r,
                Err(e) => {
                    warn!(error = %e, "failed to decode auth callout request; publishing empty reply so the NATS server fails the connect fast");
                    // The NATS server keeps the request open until it gets a
                    // reply on the inbox. An empty payload is treated as a
                    // malformed authorization response and the server
                    // immediately denies the connect — far better than the
                    // client stalling until the server-side timeout.
                    if let Err(pub_err) = self.client.publish(reply.clone(), Vec::new().into()).await {
                        error!(error = %pub_err, "failed to publish empty denial reply for undecodable request");
                    }
                    continue;
                }
            };

            let dispatcher = Arc::clone(&self.dispatcher);
            let client = self.client.clone();
            let wire = Arc::clone(&self.wire);

            tokio::spawn(async move {
                match dispatcher.dispatch(request.clone()).await {
                    Ok(user_jwt) => {
                        if let Err(e) = publish_success(&client, &reply, &wire, &request, user_jwt).await {
                            error!(error = %e, "failed to publish auth callout response");
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "auth callout denied; sending error reply");
                        if let Err(pub_err) = publish_denial(&client, &reply, &wire, &request, e.to_string()).await {
                            error!(error = %pub_err, "failed to publish auth callout denial");
                        }
                    }
                }
            });
        }

        warn!("auth callout NATS subscription closed");
        Ok(())
    }
}

async fn publish_success(
    client: &async_nats::Client,
    reply: &str,
    wire: &AuthCalloutWireCodec,
    request: &crate::wire::ServerAuthRequestClaims,
    user_jwt: crate::jwt::MintedUserJwt,
) -> Result<(), AuthCalloutError> {
    let payload = wire.encode_success(request, user_jwt)?;
    client
        .publish(reply.to_string(), payload.into())
        .await
        .map_err(|e| AuthCalloutError::Reply(e.to_string()))
}

async fn publish_denial(
    client: &async_nats::Client,
    reply: &str,
    wire: &AuthCalloutWireCodec,
    request: &crate::wire::ServerAuthRequestClaims,
    message: String,
) -> Result<(), AuthCalloutError> {
    let payload = wire.encode_denial(request, message)?;
    client
        .publish(reply.to_string(), payload.into())
        .await
        .map_err(|e| AuthCalloutError::Reply(e.to_string()))
}
