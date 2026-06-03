//! NATS service: subscribes to AAuth subjects and dispatches to `PersonCore`.

use std::sync::Arc;

use async_nats::HeaderMap;
use futures::StreamExt;
use jsonwebtoken::jwk::JwkSet;
use tracing::{error, warn};
use trogon_aauth_verify::JwksResolver;
use trogon_aauth_verify::time_source::TimeSource;

use crate::core::{BootstrapRequest, PersonCore, PersonError, TokenRequest};
use crate::policy::ConsentPolicy;
use crate::store::PersonStore;

#[derive(Clone, Debug)]
pub struct NatsConfig {
    pub ps_id: String,
    pub subject_prefix: String,
    pub queue_group: String,
}

impl NatsConfig {
    #[must_use]
    pub fn default_for(ps_id: impl Into<String>) -> Self {
        let id = ps_id.into();
        let queue = format!("trogon-aauth-person-{id}");
        Self {
            ps_id: id,
            subject_prefix: "aauth".into(),
            queue_group: queue,
        }
    }

    #[must_use]
    pub fn subject(&self, name: &str) -> String {
        format!("{}.{}.{}", self.subject_prefix, self.ps_id, name)
    }
}

pub struct NatsService<R: JwksResolver, S: PersonStore, P: ConsentPolicy, C: TimeSource> {
    pub client: async_nats::Client,
    pub core: Arc<PersonCore<R, S, P, C>>,
    pub jwks: JwkSet,
    pub cfg: NatsConfig,
}

impl<R, S, P, C> NatsService<R, S, P, C>
where
    R: JwksResolver + 'static,
    S: PersonStore + 'static,
    P: ConsentPolicy + 'static,
    C: TimeSource + Clone + 'static,
{
    /// Run all NATS subscriptions. Returns when the connection drops.
    pub async fn run(self) -> Result<(), async_nats::Error> {
        let bootstrap = self.cfg.subject("bootstrap");
        let token = self.cfg.subject("token");
        let jwks = self.cfg.subject("jwks.get");

        let queue = self.cfg.queue_group.clone();
        let mut sub_bootstrap = self.client.queue_subscribe(bootstrap, queue.clone()).await?;
        let mut sub_token = self.client.queue_subscribe(token, queue.clone()).await?;
        let mut sub_jwks = self.client.queue_subscribe(jwks, queue).await?;

        let core_b = self.core.clone();
        let client_b = self.client.clone();
        let jwks_v = self.jwks.clone();
        let client_j = self.client.clone();

        let bootstrap_loop = tokio::spawn(async move {
            while let Some(msg) = sub_bootstrap.next().await {
                let core = core_b.clone();
                let client = client_b.clone();
                tokio::spawn(async move {
                    handle_bootstrap(core, client, msg).await;
                });
            }
        });
        let core_t = self.core.clone();
        let client_t = self.client.clone();
        let token_loop = tokio::spawn(async move {
            while let Some(msg) = sub_token.next().await {
                let core = core_t.clone();
                let client = client_t.clone();
                tokio::spawn(async move {
                    handle_token(core, client, msg).await;
                });
            }
        });
        let jwks_loop = tokio::spawn(async move {
            while let Some(msg) = sub_jwks.next().await {
                let jwks_v = jwks_v.clone();
                let client = client_j.clone();
                tokio::spawn(async move {
                    handle_jwks(jwks_v, client, msg).await;
                });
            }
        });

        let (_, _, _) = tokio::join!(bootstrap_loop, token_loop, jwks_loop);
        Ok(())
    }
}

async fn handle_bootstrap<R, S, P, C>(
    core: Arc<PersonCore<R, S, P, C>>,
    client: async_nats::Client,
    msg: async_nats::Message,
) where
    R: JwksResolver,
    S: PersonStore,
    P: ConsentPolicy,
    C: TimeSource,
{
    let reply = match msg.reply.clone() {
        Some(r) => r,
        None => {
            warn!("bootstrap without reply subject");
            return;
        }
    };
    let req: BootstrapRequest = match serde_json::from_slice(&msg.payload) {
        Ok(r) => r,
        Err(e) => {
            send_error(&client, reply, 400, format!("bad request: {e}")).await;
            return;
        }
    };
    match core.bootstrap(req).await {
        Ok(resp) => send_json(&client, reply, &resp).await,
        Err(e) => send_person_error(&client, reply, e).await,
    }
}

async fn handle_token<R, S, P, C>(
    core: Arc<PersonCore<R, S, P, C>>,
    client: async_nats::Client,
    msg: async_nats::Message,
) where
    R: JwksResolver,
    S: PersonStore,
    P: ConsentPolicy,
    C: TimeSource,
{
    let reply = match msg.reply.clone() {
        Some(r) => r,
        None => {
            warn!("token exchange without reply subject");
            return;
        }
    };
    let req: TokenRequest = match serde_json::from_slice(&msg.payload) {
        Ok(r) => r,
        Err(e) => {
            send_error(&client, reply, 400, format!("bad request: {e}")).await;
            return;
        }
    };
    match core.exchange(req).await {
        Ok(resp) => send_json(&client, reply, &resp).await,
        Err(e) => send_person_error(&client, reply, e).await,
    }
}

async fn handle_jwks(jwks: JwkSet, client: async_nats::Client, msg: async_nats::Message) {
    if let Some(reply) = msg.reply.clone() {
        let bytes = match serde_json::to_vec(&jwks) {
            Ok(b) => b,
            Err(e) => {
                error!(error = %e, "jwks serialize");
                return;
            }
        };
        let _ = client.publish(reply, bytes.into()).await;
    }
}

async fn send_json<T: serde::Serialize>(client: &async_nats::Client, reply: async_nats::Subject, body: &T) {
    let bytes = match serde_json::to_vec(body) {
        Ok(b) => b,
        Err(e) => {
            error!(error = %e, "serialize response");
            return;
        }
    };
    let _ = client.publish(reply, bytes.into()).await;
}

async fn send_person_error(client: &async_nats::Client, reply: async_nats::Subject, e: PersonError) {
    let code = match &e {
        PersonError::BadRequest(_) => 400,
        PersonError::ResourceTokenInvalid(_) => 401,
        PersonError::ConsentDenied(_) => 403,
        PersonError::RequiresInteraction { .. } => 202,
        PersonError::Store(_) | PersonError::Encode(_) => 500,
    };
    send_error(client, reply, code, e.to_string()).await;
}

async fn send_error(client: &async_nats::Client, reply: async_nats::Subject, code: u16, message: String) {
    let mut h = HeaderMap::new();
    h.insert("Nats-Service-Error-Code", code.to_string().as_str());
    h.insert("Nats-Service-Error", message.as_str());
    let _ = client.publish_with_headers(reply, h, Vec::new().into()).await;
}
