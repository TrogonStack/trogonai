use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Mutex;
use trogon_sandbox_contract::{LeaseRequest, TenantId};
use trogon_sandbox_protocol::Provider;

use super::{Gate, ORIGINAL_UID, first_lease, second_lease, serve_agent, tenant_atespace, worker_behind};
use crate::actor_index::memory::MemoryActorIndex;
use crate::actor_index::{ActorEntry, ActorIndex, ActorIndexError, ActorIndexResult};
use crate::adapter::SubstrateProvider;
use crate::adapter::tests_support::{FakeControl, TestResult, lease_request};
use crate::agent::ActorTarget;
use crate::ate_client::ResourceRevision;
use crate::ate_client::{AteError, AteResult, Control};
use crate::naming::ActorName;
use crate::proto;

/// A control plane whose next `DeleteActor` waits at a gate, and which refuses to delete an actor
/// that is not suspended, as the real one does.
struct GatedControl {
    inner: Arc<FakeControl>,
    delete_gate: Mutex<Option<Gate>>,
}

#[async_trait]
impl Control for GatedControl {
    async fn create_atespace(&self, atespace: &str) -> AteResult<proto::Atespace> {
        self.inner.create_atespace(atespace).await
    }

    async fn get_actor(&self, actor: proto::ObjectRef) -> AteResult<proto::Actor> {
        self.inner.get_actor(actor).await
    }

    async fn create_actor(&self, actor: proto::Actor) -> AteResult<proto::Actor> {
        self.inner.create_actor(actor).await
    }

    async fn resume_actor(&self, actor: proto::ObjectRef) -> AteResult<proto::Actor> {
        self.inner.resume_actor(actor).await
    }

    async fn suspend_actor(&self, actor: proto::ObjectRef) -> AteResult<proto::Actor> {
        self.inner.suspend_actor(actor).await
    }

    async fn delete_actor(&self, actor: ResourceRevision) -> AteResult<proto::Actor> {
        let gate = self.delete_gate.lock().await.take();
        if let Some(gate) = gate {
            let _ = gate.entered.send(());
            gate.proceed.await.map_err(|_| AteError::Unavailable {
                message: "owned deletion gate closed".into(),
            })?;
        }
        let current = self.inner.get_actor(actor.resource().clone()).await?;
        if current
            .status
            .as_ref()
            .is_none_or(|status| status.state() != proto::ActorState::Suspended)
        {
            return Err(AteError::FailedPrecondition {
                message: "DeleteActor requires SUSPENDED".into(),
            });
        }
        self.inner.delete_actor(actor).await
    }

    async fn list_actors(&self, atespace: &str) -> AteResult<Vec<proto::Actor>> {
        self.inner.list_actors(atespace).await
    }

    async fn create_actor_template(&self, template: proto::ActorTemplate) -> AteResult<proto::ActorTemplate> {
        self.inner.create_actor_template(template).await
    }

    async fn get_actor_template(&self, template: proto::ObjectRef) -> AteResult<proto::ActorTemplate> {
        self.inner.get_actor_template(template).await
    }

    async fn delete_actor_template(&self, template: ResourceRevision) -> AteResult<()> {
        self.inner.delete_actor_template(template).await
    }
}

/// An index that fails the exchange recording an actor's UID once, as a storage outage would.
struct BindingIndex {
    inner: MemoryActorIndex,
    fail_binding: AtomicBool,
}

#[async_trait]
impl ActorIndex for BindingIndex {
    async fn put(&self, entry: &ActorEntry) -> ActorIndexResult<()> {
        self.inner.put(entry).await
    }

    async fn get(&self, atespace: &str, actor: &str) -> ActorIndexResult<Option<ActorEntry>> {
        self.inner.get(atespace, actor).await
    }

    async fn delete(&self, atespace: &str, actor: &str) -> ActorIndexResult<()> {
        self.inner.delete(atespace, actor).await
    }

    async fn all(&self) -> ActorIndexResult<Vec<ActorEntry>> {
        self.inner.all().await
    }

    async fn compare_exchange(
        &self,
        expected: Option<&ActorEntry>,
        replacement: Option<&ActorEntry>,
    ) -> ActorIndexResult<bool> {
        let binds_uid = expected
            .and_then(|entry| entry.owner.as_ref())
            .is_some_and(|owner| owner.actor_uid.is_none())
            && replacement
                .and_then(|entry| entry.owner.as_ref())
                .is_some_and(|owner| owner.actor_uid.is_some());
        if binds_uid && self.fail_binding.swap(false, Ordering::SeqCst) {
            return Err(ActorIndexError::storage(std::io::Error::other(
                "owned post-allocation UID binding outage",
            )));
        }
        self.inner.compare_exchange(expected, replacement).await
    }
}

struct Fixture {
    control: Arc<GatedControl>,
    index: Arc<BindingIndex>,
    first: SubstrateProvider,
    second: SubstrateProvider,
    request: LeaseRequest,
    tenant: TenantId,
    actor: proto::ObjectRef,
    binds: Arc<AtomicUsize>,
    _agent: crate::agent::ActorAgent,
}

async fn fixture(fail_binding: bool) -> Result<Fixture, Box<dyn std::error::Error>> {
    let index = Arc::new(BindingIndex {
        inner: MemoryActorIndex::default(),
        fail_binding: AtomicBool::new(fail_binding),
    });
    let atespace = tenant_atespace()?;
    let actor = ActorName::for_named_lease(&atespace, "custody-failure");
    let binds = Arc::new(AtomicUsize::new(0));
    let agent = serve_agent(
        ActorTarget::new(atespace.clone(), actor.clone(), 8110),
        binds.clone(),
        Arc::new(Mutex::new(None)),
    )
    .await?;
    let control = Arc::new(GatedControl {
        inner: Arc::new(FakeControl::running().with_uid(ORIGINAL_UID).with_transitions()),
        delete_gate: Mutex::new(None),
    });
    let first = worker_behind(&agent, control.clone(), index.clone(), "custody-first")?;
    let second = worker_behind(&agent, control.clone(), index.clone(), "custody-second")?;
    let request = lease_request(Some("custody-failure"), true)?;
    Ok(Fixture {
        control,
        index,
        first,
        second,
        tenant: request.tenant.clone(),
        request,
        actor: proto::ObjectRef {
            atespace: atespace.as_str().into(),
            name: actor.as_str().into(),
        },
        binds,
        _agent: agent,
    })
}

#[tokio::test]
async fn authority_retained_purge_claims_cleanup_before_a_replica_can_connect() -> TestResult {
    let fixture = fixture(false).await?;
    let (first_id, second_id) = (first_lease(), second_lease());
    Provider::lease(&fixture.first, &fixture.request, &first_id).await?;
    Provider::cancel(&fixture.first, &first_id).await?;
    let parked = fixture
        .index
        .get(&fixture.actor.atespace, &fixture.actor.name)
        .await?
        .ok_or("the parked actor lost its claim")?;
    assert!(parked.retained);
    assert_eq!(fixture.binds.load(Ordering::SeqCst), 1);
    let (gate, controls) = Gate::new();
    *fixture.control.delete_gate.lock().await = Some(gate);
    let fixture = Arc::new(fixture);
    let purge = tokio::spawn({
        let fixture = fixture.clone();
        async move { fixture.first.purge_named(&fixture.tenant, "custody-failure").await }
    });
    tokio::time::timeout(Duration::from_secs(5), controls.entered).await??;
    let deleting_claim = fixture.index.get(&fixture.actor.atespace, &fixture.actor.name).await?;

    let connected = Provider::lease(&fixture.second, &fixture.request, &second_id).await;

    let binds = fixture.binds.load(Ordering::SeqCst);
    let _ = controls.proceed.send(());
    let purged = tokio::time::timeout(Duration::from_secs(5), purge).await??;
    Provider::purge(&fixture.first, &first_id).await?;
    Provider::purge(&fixture.second, &second_id).await?;
    assert!(fixture.index.all().await?.is_empty());
    assert!(matches!(
        fixture.control.get_actor(fixture.actor.clone()).await,
        Err(AteError::NotFound { .. })
    ));
    let deleting_claim = deleting_claim.ok_or("the purge released its claim before it deleted")?;
    let owner = deleting_claim.owner.as_ref().ok_or("the purge claim has no owner")?;
    assert!(!deleting_claim.retained);
    assert_eq!(owner.worker_id, *fixture.first.worker_id());
    assert_ne!(
        Some(&owner.claim_id),
        parked.owner.as_ref().map(|prior| &prior.claim_id)
    );
    assert!(connected.is_err());
    assert_eq!(binds, 1);
    assert!(purged.is_ok());
    Ok(())
}

#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(error_string_comparison, reason = "asserts the injected outage reaches the caller")
)]
#[tokio::test]
async fn authority_post_create_binding_failure_keeps_the_exact_uid_available_for_cleanup() -> TestResult {
    let fixture = fixture(true).await?;
    let lease_id = first_lease();

    let creation = Provider::lease(&fixture.first, &fixture.request, &lease_id).await;

    let error = creation.err().ok_or("the lease must fail after allocation")?;
    assert!(
        error.to_string().contains("post-allocation UID binding outage"),
        "{error}"
    );
    let observed = fixture.control.get_actor(fixture.actor.clone()).await?;
    assert_eq!(observed.metadata.ok_or("no metadata")?.uid, ORIGINAL_UID);
    assert_eq!(fixture.binds.load(Ordering::SeqCst), 0);
    assert!(fixture.first.active_lease(&lease_id).await.is_some());

    Provider::purge(&fixture.first, &lease_id).await?;

    assert!(matches!(
        fixture.control.get_actor(fixture.actor.clone()).await,
        Err(AteError::NotFound { .. })
    ));
    assert!(
        fixture
            .index
            .get(&fixture.actor.atespace, &fixture.actor.name)
            .await?
            .is_none()
    );
    assert_eq!(fixture.control.inner.delete_count(), 1);
    Ok(())
}
