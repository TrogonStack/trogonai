use std::sync::Arc;
use std::time::Duration;

use async_nats::Client;
use async_nats::jetstream::{self, stream};
use axum::Router;
use bytes::Bytes;
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use trogon_nats::test_support::JetStreamTestServer;
use trogon_sandbox_contract::{LeaseId, Port, TenantId};
use trogon_sandbox_protocol::{AuthorityConfig, AuthorityMode};
use trogon_std::env::InMemoryEnv;

use crate::actor_index::memory::MemoryActorIndex;
use crate::actor_index::{ActorClaimId, ActorEntry, ActorIndex, ActorOwner, ActorUid};
use crate::adapter::tests_support::{FakeControl, WarmTemplates, config};
use crate::adapter::{ActiveLease, SubstrateProvider};
use crate::agent::tests::{agent_router, serve, target};
use crate::naming::{ActorName, Atespace};
use crate::template::TemplateRegistry;
use crate::worker::{WorkerError, run_with_authority};

pub(super) type TestResult = Result<(), Box<dyn std::error::Error>>;

/// The lease whose actor the fake agent answers for.
pub(super) const HELD: &str = "01J0";

/// A lease this worker holds whose actor the fake agent refuses to serve.
pub(super) const MISROUTED: &str = "01J1";

/// A lease nobody holds.
pub(super) const UNHELD: &str = "01J2";

pub(super) const WAIT: Duration = Duration::from_secs(10);

pub(super) const UID: &str = "00000000-0000-4000-8000-000000000001";

pub(super) const EXPOSED_PORT: u16 = 8080;

/// A worker serving over a real NATS server, with a fake agent behind the held leases.
pub(super) struct Rig {
    pub(super) _server: JetStreamTestServer,
    pub(super) client: Client,
    pub(super) js: jetstream::Context,
    pub(super) provider: Arc<SubstrateProvider>,
    pub(super) control: Arc<FakeControl>,
    pub(super) index: Arc<MemoryActorIndex>,
    stop: CancellationToken,
    task: JoinHandle<Result<(), WorkerError>>,
}

/// Opens an evidence stream so published chunks, exit markers and lifecycle events are acknowledged.
pub(super) async fn create_evidence_stream(js: &jetstream::Context) -> TestResult {
    js.create_stream(stream::Config {
        name: "EVIDENCE".into(),
        subjects: vec!["sandbox.evidence.>".into()],
        ..Default::default()
    })
    .await?;
    Ok(())
}

pub(super) fn body(value: &Value) -> Result<Bytes, serde_json::Error> {
    Ok(Bytes::from(serde_json::to_vec(value)?))
}

/// Builds a provider that serves its actors through `agent` and holds `leases`.
pub(super) async fn provider_holding(
    router: Router,
    leases: &[&str],
) -> Result<(Arc<SubstrateProvider>, Arc<FakeControl>, Arc<MemoryActorIndex>), Box<dyn std::error::Error>> {
    let agent = serve(target()?, router).await?;
    let mut config = config()?;
    config.router_endpoint = agent_router(&agent);
    let control = Arc::new(FakeControl::running().with_uid(UID).with_transitions());
    let index = Arc::new(MemoryActorIndex::default());
    let provider = Arc::new(SubstrateProvider::new(
        config,
        control.clone(),
        TemplateRegistry::new(Arc::new(WarmTemplates)),
        index.clone(),
        &InMemoryEnv::new(),
    )?);
    for lease in leases {
        hold(&provider, &index, lease, None).await?;
    }
    Ok((provider, control, index))
}

/// Registers `lease` as held by `provider`, with an actor the control plane confirms.
pub(super) async fn hold(
    provider: &SubstrateProvider,
    index: &MemoryActorIndex,
    lease: &str,
    exposed_port: Option<u16>,
) -> TestResult {
    let lease_id = LeaseId::new(lease);
    let atespace = Atespace::for_tenant(&TenantId::new("tenant-a")?)?;
    let actor = ActorName::for_lease(&lease_id)?;
    let entry = ActorEntry {
        atespace: atespace.as_str().into(),
        actor: actor.as_str().into(),
        lease_id: lease_id.clone(),
        named: false,
        retained: false,
        owner: Some(ActorOwner {
            worker_id: provider.worker_id().clone(),
            claim_id: ActorClaimId::generate(),
            actor_uid: Some(ActorUid::try_from(UID.to_string())?),
        }),
    };
    index.put(&entry).await?;
    provider
        .adopt(
            lease_id,
            ActiveLease {
                atespace,
                actor,
                named: false,
                persistent: false,
                exposed_port: exposed_port.map(Port::new).transpose()?,
            },
            entry,
            true,
        )
        .await?;
    Ok(())
}

impl Rig {
    pub(super) async fn start(router: Router) -> Result<Self, Box<dyn std::error::Error>> {
        let server = JetStreamTestServer::start().await;
        let client = server.client().await;
        let js = server.jetstream().await;
        create_evidence_stream(&js).await?;
        let (provider, control, index) = provider_holding(router, &[HELD, MISROUTED]).await?;
        hold(&provider, &index, "01J3", Some(EXPOSED_PORT)).await?;
        let stop = CancellationToken::new();
        let task = tokio::spawn(run_with_authority(
            client.clone(),
            js.clone(),
            provider.clone(),
            provider.worker_id().clone(),
            stop.clone(),
            AuthorityConfig::new(AuthorityMode::Off),
        ));
        let rig = Self {
            _server: server,
            client,
            js,
            provider,
            control,
            index,
            stop,
            task,
        };
        rig.until_serving().await?;
        Ok(rig)
    }

    /// Waits until the worker answers, since its subscriptions open in the background.
    async fn until_serving(&self) -> TestResult {
        let subject = trogon_sandbox_protocol::subjects::provider_get_domain(
            trogon_sandbox_contract::ProviderKind::Substrate,
            &LeaseId::new(HELD),
        );
        let probe = body(&serde_json::json!({"lease_id": HELD, "port": 1}))?;
        tokio::time::timeout(WAIT, async {
            loop {
                if self.client.request(subject.clone(), probe.clone()).await.is_ok() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        Ok(())
    }

    /// Sends `request` and decodes the reply.
    pub(super) async fn ask<Res: DeserializeOwned>(
        &self,
        subject: String,
        request: &Value,
    ) -> Result<Res, Box<dyn std::error::Error>> {
        let reply = tokio::time::timeout(WAIT, self.client.request(subject, body(request)?)).await??;
        Ok(serde_json::from_slice(&reply.payload)?)
    }

    /// Whether the worker leaves `request` unanswered.
    pub(super) async fn unanswered(&self, subject: String, payload: Bytes) -> Result<bool, Box<dyn std::error::Error>> {
        let outcome = tokio::time::timeout(Duration::from_millis(400), self.client.request(subject, payload)).await;
        Ok(outcome.is_err())
    }

    pub(super) async fn finish(self) -> TestResult {
        self.stop.cancel();
        tokio::time::timeout(WAIT, self.task).await???;
        Ok(())
    }
}
