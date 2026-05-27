use std::sync::Arc;

use futures_util::StreamExt;
use tokio::sync::{RwLock, watch};
use tracing::{info, warn};

use crate::jwks::{Jwks, REQREP_QUEUE_GROUP, REQREP_SUBJECT};

#[derive(Clone)]
pub struct ReqRepState {
    jwks: Arc<RwLock<Jwks>>,
}

pub fn current_jwks_json(state: &ReqRepState) -> Result<Vec<u8>, serde_json::Error> {
    let jwks = state
        .jwks
        .try_read()
        .map(|guard| guard.clone())
        .unwrap_or_else(|_| state.jwks.blocking_read().clone());
    jwks.to_json()
}

pub async fn run_reqrep_publisher(
    nats_url: &str,
    mut watch_rx: watch::Receiver<Jwks>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let client = async_nats::connect(nats_url).await?;
    let state = ReqRepState {
        jwks: Arc::new(RwLock::new(watch_rx.borrow().clone())),
    };
    let cache = Arc::clone(&state.jwks);
    tokio::spawn(async move {
        while watch_rx.changed().await.is_ok() {
            let jwks = watch_rx.borrow().clone();
            *cache.write().await = jwks;
        }
    });

    let mut subscriber = client
        .queue_subscribe(REQREP_SUBJECT, REQREP_QUEUE_GROUP.to_owned())
        .await?;

    info!(
        subject = REQREP_SUBJECT,
        queue = REQREP_QUEUE_GROUP,
        "JWKS request/reply publisher ready"
    );

    while let Some(message) = subscriber.next().await {
        let payload = match current_jwks_json(&state) {
            Ok(payload) => payload,
            Err(err) => {
                warn!(error = %err, "failed to serialize JWKS for request/reply");
                continue;
            }
        };
        if let Some(reply) = message.reply
            && let Err(err) = client.publish(reply, payload.into()).await
        {
            warn!(error = %err, "failed to reply with JWKS");
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use jsonwebtoken::jwk::{AlgorithmParameters, CommonParameters, Jwk, PublicKeyUse, RSAKeyParameters, RSAKeyType};

    use super::*;

    #[test]
    fn replies_with_in_memory_jwks_json() {
        let jwks = Jwks {
            keys: vec![Jwk {
                common: CommonParameters {
                    public_key_use: Some(PublicKeyUse::Signature),
                    key_id: Some("reqrep-kid".into()),
                    ..Default::default()
                },
                algorithm: AlgorithmParameters::RSA(RSAKeyParameters {
                    key_type: RSAKeyType::RSA,
                    n: "abc".into(),
                    e: "AQAB".into(),
                }),
            }],
        };
        let state = ReqRepState {
            jwks: Arc::new(RwLock::new(jwks.clone())),
        };
        let payload = current_jwks_json(&state).expect("serialize");
        let parsed: Jwks = serde_json::from_slice(&payload).expect("parse");
        assert_eq!(parsed, jwks);
    }
}
