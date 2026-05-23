use std::sync::Arc;

use futures::StreamExt as _;
use tracing::{error, info, warn};

use crate::dispatcher::{AuthCalloutRequest, AuthCalloutResponse, AuthDispatcher};
use crate::error::AuthCalloutError;

/// NATS subject the server uses for auth callout requests.
const AUTH_CALLOUT_SUBJECT: &str = "$SYS.REQ.USER.AUTH";

/// Subscriber loop that listens on `$SYS.REQ.USER.AUTH` and delegates each
/// request to the injected `AuthDispatcher`.
///
/// Keeping I/O (NATS) and decision logic (dispatcher) separate makes the loop
/// unit-testable without a live server.
pub struct Subscriber<D> {
    client: async_nats::Client,
    dispatcher: Arc<D>,
}

impl<D: AuthDispatcher> Subscriber<D> {
    pub fn new(client: async_nats::Client, dispatcher: D) -> Self {
        Self {
            client,
            dispatcher: Arc::new(dispatcher),
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

            let request: AuthCalloutRequest = match serde_json::from_slice(&msg.payload) {
                Ok(r) => r,
                Err(e) => {
                    warn!(error = %e, "failed to deserialize auth callout request; dropping");
                    continue;
                }
            };

            let dispatcher = Arc::clone(&self.dispatcher);
            let client = self.client.clone();

            tokio::spawn(async move {
                match dispatcher.dispatch(request).await {
                    Ok(response) => {
                        if let Err(e) = publish_response(&client, &reply, &response).await {
                            error!(error = %e, "failed to publish auth callout response");
                        }
                    }
                    Err(e) => {
                        warn!(error = %e, "auth callout denied; sending error reply");
                        publish_denial(&client, &reply).await;
                    }
                }
            });
        }

        warn!("auth callout NATS subscription closed");
        Ok(())
    }
}

async fn publish_response(
    client: &async_nats::Client,
    reply: &str,
    response: &AuthCalloutResponse,
) -> Result<(), AuthCalloutError> {
    let payload = serde_json::to_vec(response).map_err(AuthCalloutError::Serialize)?;
    client
        .publish(reply.to_string(), payload.into())
        .await
        .map_err(|e| AuthCalloutError::Reply(e.to_string()))
}

async fn publish_denial(client: &async_nats::Client, reply: &str) {
    // A minimal denial response — the exact error encoding for the NATS auth callout
    // protocol is TODO(spec) pending wire format confirmation.
    let payload = serde_json::json!({ "error": "authorization denied" });
    if let Ok(bytes) = serde_json::to_vec(&payload) {
        let _ = client.publish(reply.to_string(), bytes.into()).await;
    }
}
