//! NATS admin RPC for Workload Identity Federation records and exchange.
//!
//! Provides CRUD-style operations over [`crate::wif::WifStore`] plus the
//! mint-completing `wif.exchange` subject. All operations are JSON over
//! request/reply; errors map to `Nats-Service-Error-Code` headers.
//!
//! Subjects (under `aauth.<ps_id>`):
//! - `wif.providers.create` – upsert a provider record
//! - `wif.providers.get` – fetch by id
//! - `wif.providers.list` – list all providers
//! - `wif.providers.disable` – set `lifecycle_state = Disabled`
//! - `wif.mappings.create` – upsert a service-account mapping
//! - `wif.mappings.get` – fetch a mapping by id
//! - `wif.mappings.list` – list mappings for a provider
//! - `wif.mappings.disable` – set `lifecycle_state = Disabled`
//! - `wif.exchange` – RFC 8693 token exchange ending in `aa-auth+jwt`

use std::sync::Arc;

use async_nats::HeaderMap;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use tracing::{error, warn};
use trogon_aauth_verify::JwksResolver;
use trogon_aauth_verify::time_source::TimeSource;

use crate::core::{PersonCore, PersonError};
use crate::nats::NatsConfig;
use crate::policy::ConsentPolicy;
use crate::store::PersonStore;
use crate::wif::{
    LifecycleState, ServiceAccountMappingRecord, WifStore, WorkloadIdentityProviderRecord,
};
use crate::wif_exchange::{WifExchangeError, WifExchangeRequest, WifExchangeService};

#[derive(Debug, Serialize, Deserialize)]
pub struct GetById {
    pub id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ListMappingsForProvider {
    pub provider_id: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct ProviderList {
    pub providers: Vec<WorkloadIdentityProviderRecord>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct MappingList {
    pub mappings: Vec<ServiceAccountMappingRecord>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Ack {
    pub id: String,
    pub lifecycle_state: LifecycleState,
}

pub struct WifAdminService<R, S, P, C, W, J>
where
    R: JwksResolver,
    S: PersonStore,
    P: ConsentPolicy,
    C: TimeSource,
    W: WifStore,
    J: JwksResolver,
{
    pub client: async_nats::Client,
    pub core: Arc<PersonCore<R, S, P, C>>,
    pub wif_store: Arc<W>,
    pub exchange: Arc<WifExchangeService<Arc<W>, J, C>>,
    pub clock: C,
    pub cfg: NatsConfig,
}

impl<R, S, P, C, W, J> WifAdminService<R, S, P, C, W, J>
where
    R: JwksResolver + 'static,
    S: PersonStore + 'static,
    P: ConsentPolicy + 'static,
    C: TimeSource + Clone + 'static,
    W: WifStore + 'static,
    J: JwksResolver + 'static,
{
    pub async fn run(self) -> Result<(), async_nats::Error> {
        let queue = self.cfg.queue_group.clone();

        let providers_create = self.cfg.subject("wif.providers.create");
        let providers_get = self.cfg.subject("wif.providers.get");
        let providers_list = self.cfg.subject("wif.providers.list");
        let providers_disable = self.cfg.subject("wif.providers.disable");
        let mappings_create = self.cfg.subject("wif.mappings.create");
        let mappings_get = self.cfg.subject("wif.mappings.get");
        let mappings_list = self.cfg.subject("wif.mappings.list");
        let mappings_disable = self.cfg.subject("wif.mappings.disable");
        let exchange_sub = self.cfg.subject("wif.exchange");

        let mut sub_pc = self.client.queue_subscribe(providers_create, queue.clone()).await?;
        let mut sub_pg = self.client.queue_subscribe(providers_get, queue.clone()).await?;
        let mut sub_pl = self.client.queue_subscribe(providers_list, queue.clone()).await?;
        let mut sub_pd = self.client.queue_subscribe(providers_disable, queue.clone()).await?;
        let mut sub_mc = self.client.queue_subscribe(mappings_create, queue.clone()).await?;
        let mut sub_mg = self.client.queue_subscribe(mappings_get, queue.clone()).await?;
        let mut sub_ml = self.client.queue_subscribe(mappings_list, queue.clone()).await?;
        let mut sub_md = self.client.queue_subscribe(mappings_disable, queue.clone()).await?;
        let mut sub_ex = self.client.queue_subscribe(exchange_sub, queue).await?;

        let store = self.wif_store.clone();
        let client = self.client.clone();
        let clock = self.clock.clone();
        let pc_loop = tokio::spawn(async move {
            while let Some(msg) = sub_pc.next().await {
                let store = store.clone();
                let client = client.clone();
                let clock = clock.clone();
                tokio::spawn(async move {
                    handle_provider_create(store, client, clock, msg).await;
                });
            }
        });

        let store = self.wif_store.clone();
        let client = self.client.clone();
        let pg_loop = tokio::spawn(async move {
            while let Some(msg) = sub_pg.next().await {
                let store = store.clone();
                let client = client.clone();
                tokio::spawn(async move {
                    handle_provider_get(store, client, msg).await;
                });
            }
        });

        let store = self.wif_store.clone();
        let client = self.client.clone();
        let pl_loop = tokio::spawn(async move {
            while let Some(msg) = sub_pl.next().await {
                let store = store.clone();
                let client = client.clone();
                tokio::spawn(async move {
                    handle_provider_list(store, client, msg).await;
                });
            }
        });

        let store = self.wif_store.clone();
        let client = self.client.clone();
        let clock = self.clock.clone();
        let pd_loop = tokio::spawn(async move {
            while let Some(msg) = sub_pd.next().await {
                let store = store.clone();
                let client = client.clone();
                let clock = clock.clone();
                tokio::spawn(async move {
                    handle_provider_disable(store, client, clock, msg).await;
                });
            }
        });

        let store = self.wif_store.clone();
        let client = self.client.clone();
        let clock = self.clock.clone();
        let mc_loop = tokio::spawn(async move {
            while let Some(msg) = sub_mc.next().await {
                let store = store.clone();
                let client = client.clone();
                let clock = clock.clone();
                tokio::spawn(async move {
                    handle_mapping_create(store, client, clock, msg).await;
                });
            }
        });

        let store = self.wif_store.clone();
        let client = self.client.clone();
        let mg_loop = tokio::spawn(async move {
            while let Some(msg) = sub_mg.next().await {
                let store = store.clone();
                let client = client.clone();
                tokio::spawn(async move {
                    handle_mapping_get(store, client, msg).await;
                });
            }
        });

        let store = self.wif_store.clone();
        let client = self.client.clone();
        let ml_loop = tokio::spawn(async move {
            while let Some(msg) = sub_ml.next().await {
                let store = store.clone();
                let client = client.clone();
                tokio::spawn(async move {
                    handle_mapping_list(store, client, msg).await;
                });
            }
        });

        let store = self.wif_store.clone();
        let client = self.client.clone();
        let clock = self.clock.clone();
        let md_loop = tokio::spawn(async move {
            while let Some(msg) = sub_md.next().await {
                let store = store.clone();
                let client = client.clone();
                let clock = clock.clone();
                tokio::spawn(async move {
                    handle_mapping_disable(store, client, clock, msg).await;
                });
            }
        });

        let exchange = self.exchange.clone();
        let core = self.core.clone();
        let client = self.client.clone();
        let ex_loop = tokio::spawn(async move {
            while let Some(msg) = sub_ex.next().await {
                let exchange = exchange.clone();
                let core = core.clone();
                let client = client.clone();
                tokio::spawn(async move {
                    handle_exchange(exchange, core, client, msg).await;
                });
            }
        });

        let _ = tokio::join!(
            pc_loop, pg_loop, pl_loop, pd_loop, mc_loop, mg_loop, ml_loop, md_loop, ex_loop
        );
        Ok(())
    }
}

async fn handle_provider_create<W: WifStore, C: TimeSource>(
    store: Arc<W>,
    client: async_nats::Client,
    clock: C,
    msg: async_nats::Message,
) {
    let Some(reply) = msg.reply.clone() else {
        warn!("wif.providers.create without reply");
        return;
    };
    let mut record: WorkloadIdentityProviderRecord = match serde_json::from_slice(&msg.payload) {
        Ok(r) => r,
        Err(e) => return send_error(&client, reply, 400, format!("bad request: {e}")).await,
    };
    let now = clock.now();
    if record.created_at == 0 {
        record.created_at = now;
    }
    record.updated_at = now;
    match store.put_provider(record.clone()).await {
        Ok(()) => send_json(&client, reply, &record).await,
        Err(e) => send_error(&client, reply, 500, format!("store: {e}")).await,
    }
}

async fn handle_provider_get<W: WifStore>(store: Arc<W>, client: async_nats::Client, msg: async_nats::Message) {
    let Some(reply) = msg.reply.clone() else {
        return;
    };
    let req: GetById = match serde_json::from_slice(&msg.payload) {
        Ok(r) => r,
        Err(e) => return send_error(&client, reply, 400, format!("bad request: {e}")).await,
    };
    match store.get_provider(&req.id).await {
        Ok(Some(r)) => send_json(&client, reply, &r).await,
        Ok(None) => send_error(&client, reply, 404, format!("provider `{}` not found", req.id)).await,
        Err(e) => send_error(&client, reply, 500, format!("store: {e}")).await,
    }
}

async fn handle_provider_list<W: WifStore>(store: Arc<W>, client: async_nats::Client, msg: async_nats::Message) {
    let Some(reply) = msg.reply.clone() else {
        return;
    };
    match store.list_providers().await {
        Ok(providers) => send_json(&client, reply, &ProviderList { providers }).await,
        Err(e) => send_error(&client, reply, 500, format!("store: {e}")).await,
    }
}

async fn handle_provider_disable<W: WifStore, C: TimeSource>(
    store: Arc<W>,
    client: async_nats::Client,
    clock: C,
    msg: async_nats::Message,
) {
    let Some(reply) = msg.reply.clone() else {
        return;
    };
    let req: GetById = match serde_json::from_slice(&msg.payload) {
        Ok(r) => r,
        Err(e) => return send_error(&client, reply, 400, format!("bad request: {e}")).await,
    };
    match store.get_provider(&req.id).await {
        Ok(Some(mut record)) => {
            record.lifecycle_state = LifecycleState::Disabled;
            record.updated_at = clock.now();
            if let Err(e) = store.put_provider(record.clone()).await {
                send_error(&client, reply, 500, format!("store: {e}")).await;
                return;
            }
            send_json(
                &client,
                reply,
                &Ack {
                    id: record.id,
                    lifecycle_state: record.lifecycle_state,
                },
            )
            .await;
        }
        Ok(None) => send_error(&client, reply, 404, format!("provider `{}` not found", req.id)).await,
        Err(e) => send_error(&client, reply, 500, format!("store: {e}")).await,
    }
}

async fn handle_mapping_create<W: WifStore, C: TimeSource>(
    store: Arc<W>,
    client: async_nats::Client,
    clock: C,
    msg: async_nats::Message,
) {
    let Some(reply) = msg.reply.clone() else {
        return;
    };
    let mut record: ServiceAccountMappingRecord = match serde_json::from_slice(&msg.payload) {
        Ok(r) => r,
        Err(e) => return send_error(&client, reply, 400, format!("bad request: {e}")).await,
    };
    let now = clock.now();
    if record.created_at == 0 {
        record.created_at = now;
    }
    record.updated_at = now;
    match store.put_mapping(record.clone()).await {
        Ok(()) => send_json(&client, reply, &record).await,
        Err(e) => send_error(&client, reply, 500, format!("store: {e}")).await,
    }
}

async fn handle_mapping_get<W: WifStore>(store: Arc<W>, client: async_nats::Client, msg: async_nats::Message) {
    let Some(reply) = msg.reply.clone() else {
        return;
    };
    let req: GetById = match serde_json::from_slice(&msg.payload) {
        Ok(r) => r,
        Err(e) => return send_error(&client, reply, 400, format!("bad request: {e}")).await,
    };
    match store.get_mapping(&req.id).await {
        Ok(Some(r)) => send_json(&client, reply, &r).await,
        Ok(None) => send_error(&client, reply, 404, format!("mapping `{}` not found", req.id)).await,
        Err(e) => send_error(&client, reply, 500, format!("store: {e}")).await,
    }
}

async fn handle_mapping_list<W: WifStore>(store: Arc<W>, client: async_nats::Client, msg: async_nats::Message) {
    let Some(reply) = msg.reply.clone() else {
        return;
    };
    let req: ListMappingsForProvider = match serde_json::from_slice(&msg.payload) {
        Ok(r) => r,
        Err(e) => return send_error(&client, reply, 400, format!("bad request: {e}")).await,
    };
    match store.list_mappings_for_provider(&req.provider_id).await {
        Ok(mappings) => send_json(&client, reply, &MappingList { mappings }).await,
        Err(e) => send_error(&client, reply, 500, format!("store: {e}")).await,
    }
}

async fn handle_mapping_disable<W: WifStore, C: TimeSource>(
    store: Arc<W>,
    client: async_nats::Client,
    clock: C,
    msg: async_nats::Message,
) {
    let Some(reply) = msg.reply.clone() else {
        return;
    };
    let req: GetById = match serde_json::from_slice(&msg.payload) {
        Ok(r) => r,
        Err(e) => return send_error(&client, reply, 400, format!("bad request: {e}")).await,
    };
    match store.get_mapping(&req.id).await {
        Ok(Some(mut record)) => {
            record.lifecycle_state = LifecycleState::Disabled;
            record.updated_at = clock.now();
            if let Err(e) = store.put_mapping(record.clone()).await {
                send_error(&client, reply, 500, format!("store: {e}")).await;
                return;
            }
            send_json(
                &client,
                reply,
                &Ack {
                    id: record.id,
                    lifecycle_state: record.lifecycle_state,
                },
            )
            .await;
        }
        Ok(None) => send_error(&client, reply, 404, format!("mapping `{}` not found", req.id)).await,
        Err(e) => send_error(&client, reply, 500, format!("store: {e}")).await,
    }
}

async fn handle_exchange<R, S, P, C, W, J>(
    exchange: Arc<WifExchangeService<Arc<W>, J, C>>,
    core: Arc<PersonCore<R, S, P, C>>,
    client: async_nats::Client,
    msg: async_nats::Message,
) where
    R: JwksResolver,
    S: PersonStore,
    P: ConsentPolicy,
    C: TimeSource,
    W: WifStore,
    J: JwksResolver,
{
    let Some(reply) = msg.reply.clone() else {
        warn!("wif.exchange without reply");
        return;
    };
    let req: WifExchangeRequest = match serde_json::from_slice(&msg.payload) {
        Ok(r) => r,
        Err(e) => return send_error(&client, reply, 400, format!("bad request: {e}")).await,
    };
    let resolved = match exchange.resolve(&req).await {
        Ok(r) => r,
        Err(e) => return send_wif_error(&client, reply, e).await,
    };
    match core.mint_wif_auth(&resolved) {
        Ok(resp) => send_json(&client, reply, &resp).await,
        Err(e) => send_person_error(&client, reply, e).await,
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

async fn send_wif_error(client: &async_nats::Client, reply: async_nats::Subject, e: WifExchangeError) {
    let code = match &e {
        WifExchangeError::UnsupportedGrantType(_) | WifExchangeError::UnsupportedSubjectTokenType(_) => 400,
        WifExchangeError::UnknownProvider(_)
        | WifExchangeError::ProviderDisabled(_)
        | WifExchangeError::UnknownServiceAccount(..)
        | WifExchangeError::NoMappingMatch
        | WifExchangeError::AmbiguousMatch(_) => 403,
        WifExchangeError::SubjectToken(_) | WifExchangeError::IssuerMismatch { .. } => 401,
        WifExchangeError::Jwks(_) | WifExchangeError::ClaimMapping(_) | WifExchangeError::Store(_) => 500,
    };
    send_error(client, reply, code, e.to_string()).await;
}

async fn send_error(client: &async_nats::Client, reply: async_nats::Subject, code: u16, message: String) {
    let mut h = HeaderMap::new();
    h.insert("Nats-Service-Error-Code", code.to_string().as_str());
    h.insert("Nats-Service-Error", message.as_str());
    let _ = client.publish_with_headers(reply, h, Vec::new().into()).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wif::{InMemoryWifStore, KeySource, MatchPattern};
    use std::collections::BTreeMap;

    #[tokio::test]
    async fn provider_round_trip_via_in_memory_store() {
        let store = Arc::new(InMemoryWifStore::new());
        let record = WorkloadIdentityProviderRecord {
            id: "wif_p".into(),
            iss: "https://iss".into(),
            audiences: vec!["aud".into()],
            key_source: KeySource::OidcDiscovery {
                issuer_url: "https://iss".into(),
            },
            claim_mappings: vec![],
            lifecycle_state: LifecycleState::Enabled,
            created_at: 1,
            updated_at: 1,
            metadata: None,
        };
        store.put_provider(record.clone()).await.unwrap();
        let got = store.get_provider("wif_p").await.unwrap().unwrap();
        assert_eq!(got, record);
    }

    #[tokio::test]
    async fn mapping_list_filters_by_provider() {
        let store = Arc::new(InMemoryWifStore::new());
        let mut attrs = BTreeMap::new();
        attrs.insert("openai.subject".into(), MatchPattern::Exact { value: "a".into() });
        let m = ServiceAccountMappingRecord {
            id: "m1".into(),
            provider_id: "wif_p".into(),
            match_attributes: attrs,
            service_account_id: "sa".into(),
            permissions: vec![],
            lifecycle_state: LifecycleState::Enabled,
            created_at: 1,
            updated_at: 1,
            metadata: None,
        };
        store.put_mapping(m).await.unwrap();
        let listed = store.list_mappings_for_provider("wif_p").await.unwrap();
        assert_eq!(listed.len(), 1);
    }
}
