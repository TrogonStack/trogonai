use std::sync::Arc;

use futures::StreamExt as _;
use tracing::{error, info, warn};

use crate::dispatcher::AuthDispatcher;
use crate::error::AuthCalloutError;
use crate::wire::AuthCalloutWireCodec;

/// NATS subject the server uses for auth callout requests.
const AUTH_CALLOUT_SUBJECT: &str = "$SYS.REQ.USER.AUTH";

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
        let mut sub = self
            .client
            .subscribe(AUTH_CALLOUT_SUBJECT)
            .await
            .map_err(|e| AuthCalloutError::Subscribe(e.to_string()))?;

        info!(subject = AUTH_CALLOUT_SUBJECT, "auth callout subscriber started");

        while let Some(msg) = sub.next().await {
            let reply = match msg.reply.clone() {
                Some(r) => r,
                None => {
                    warn!("auth callout request without reply subject; dropping");
                    continue;
                }
            };

            let request = match self
                .wire
                .decode_request(msg.payload.to_vec(), msg.headers.as_ref())
            {
                Ok(r) => r,
                Err(e) => {
                    warn!(error = %e, "failed to decode auth callout request; dropping");
                    continue;
                }
            };

            let dispatcher = Arc::clone(&self.dispatcher);
            let client = self.client.clone();
            let wire = Arc::clone(&self.wire);

            tokio::spawn(async move {
                match dispatcher.dispatch(request.clone()).await {
                    Ok(user_jwt) => {
                        if let Err(e) = publish_success(&client, &reply, &wire, &request, user_jwt).await
                        {
                            error!(error = %e, "failed to publish auth callout response");
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "auth callout denied; sending error reply");
                        if let Err(pub_err) =
                            publish_denial(&client, &reply, &wire, &request, e.to_string()).await
                        {
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
