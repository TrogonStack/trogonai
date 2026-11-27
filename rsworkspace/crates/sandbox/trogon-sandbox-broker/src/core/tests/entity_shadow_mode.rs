use super::*;
use crate::entity_shadow::{EntityShadow, InMemoryEntityEventSink};
use crate::entity_store::SandboxEntityMode;
use crate::entity_store::{
    EntityStoreError, InMemorySandboxEntityStore, NewSandboxEntity, SandboxEntityStore, Versioned,
};
use trogon_sandbox_contract::SandboxEntityId;
use trogon_sandbox_contract::{
    DeleteEntityRequest, DeleteEntityResponse, EntityLifecycleEvent, EntityRef, EntityState, GetEntityRequest,
    GetEntityResponse, ListEntitiesRequest, ListEntitiesResponse,
};
use trogon_sandbox_protocol::EntityEvent;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

fn named(key: &str) -> anyhow::Result<LeaseRequest> {
    let mut request = make_request(vec![ProviderKind::Mock])?;
    request.name = Some(LeaseName::new("workspace")?);
    request.persistent = true.into();
    request.idempotency_key = LeaseRequestKey::new(key)?;
    Ok(request)
}

fn shadowed(core: BrokerCore, store: Arc<dyn SandboxEntityStore>) -> (BrokerCore, Arc<InMemoryEntityEventSink>) {
    let sink = Arc::new(InMemoryEntityEventSink::new());
    let shadow = EntityShadow::new(
        SandboxEntityMode::Shadow,
        store,
        sink.clone(),
        Duration::from_millis(10),
    )
    .with_head_poll(Duration::from_millis(10), 100);
    (core.with_entity_shadow(Arc::new(shadow)), sink)
}

fn granted(response: LeaseResponse) -> Result<LeaseGranted, Box<dyn std::error::Error + Send + Sync>> {
    match response {
        LeaseResponse::Granted(grant) => Ok(grant),
        other => Err(format!("expected a grant, got {other:?}").into()),
    }
}

async fn release(core: &BrokerCore, lease: &LeaseId) -> TestResult {
    core.release_lease(lease.clone(), CancelIntent::Release, LeaseReleasedReason::Cancelled)
        .await?;
    Ok(())
}

async fn entity_of(store: &InMemorySandboxEntityStore) -> Result<Versioned, Box<dyn std::error::Error + Send + Sync>> {
    Ok(store
        .get_by_name(&TenantId::new("test-tenant")?, "workspace")
        .await?
        .ok_or("record")?)
}

fn new_entity(name: &str) -> anyhow::Result<NewSandboxEntity> {
    Ok(NewSandboxEntity {
        tenant: TenantId::new("test-tenant")?,
        name: Some(name.to_string()),
        created_by: None,
        spec: make_request(vec![ProviderKind::Mock])?.spec,
        metadata: SandboxTags::default(),
        drives: vec![],
        persistence_tag: None,
    })
}

fn tag_record(by: &str) -> anyhow::Result<SnapshotTagRecord> {
    Ok(SnapshotTagRecord {
        digest: test_digest()?,
        artifact_type: ArtifactType::HyperlightSnapshotV1,
        set_by: ServiceAccountId::new(by),
        set_at: SystemTime::now(),
        retention: None,
    })
}

#[tokio::test]
async fn off_mode_leaves_the_response_without_an_entity_id() -> TestResult {
    let core = make_core(mock_provider_response()?)?;
    let grant = granted(core.handle_lease_request(named("k1")?).await)?;
    assert!(grant.entity_id.is_none());
    let json = serde_json::to_value(&grant)?;
    assert!(json.get("entity_id").is_none());
    assert_eq!(core.sweep_entities().await, Default::default());
    Ok(())
}

#[tokio::test]
async fn lease_records_stay_compatible_without_an_entity_id() -> TestResult {
    let core = make_core(mock_provider_response()?)?;
    let grant = granted(core.handle_lease_request(named("k1")?).await)?;
    let record = core.active_leases.get(&grant.lease_id).await?.ok_or("missing record")?;
    assert!(record.entity_id.is_none());
    let json = serde_json::to_string(&record)?;
    assert!(!json.contains("entity_id"));
    let back: LeaseRecord = serde_json::from_str(&json)?;
    assert_eq!(back.lease_id, record.lease_id);

    let store = Arc::new(InMemorySandboxEntityStore::new());
    let (core, _) = shadowed(make_core(mock_provider_response()?)?, store);
    let grant = granted(core.handle_lease_request(named("k1")?).await)?;
    let record = core.active_leases.get(&grant.lease_id).await?.ok_or("missing record")?;
    assert_eq!(record.entity_id, grant.entity_id);
    assert!(record.entity_id.is_some());
    Ok(())
}

#[tokio::test]
async fn off_mode_creates_no_entity_buckets() -> TestResult {
    let server = crate::test_support::LocalNats::start().await;
    let core = make_core(mock_provider_response()?)?;
    granted(core.handle_lease_request(named("k1")?).await)?;
    let js = async_nats::jetstream::new(server.client.clone());
    assert!(js.get_key_value("sandbox-entities").await.is_err());
    assert!(js.get_key_value("sandbox-entity-names").await.is_err());
    assert!(js.get_stream("sandbox-entity-events").await.is_err());
    Ok(())
}

#[tokio::test]
async fn the_entity_id_is_stable_across_stop_and_start() -> TestResult {
    let store = Arc::new(InMemorySandboxEntityStore::new());
    let (core, sink) = shadowed(make_core(mock_provider_response()?)?, store.clone());

    let first = granted(core.handle_lease_request(named("k1")?).await)?;
    let id = first.entity_id.clone().ok_or("entity id")?;
    let entity = entity_of(&store).await?.record;
    assert_eq!(entity.entity_id, id);
    assert_eq!(entity.generation, 1);
    assert_eq!(
        entity.state,
        EntityState::Running {
            lease: first.lease_id.clone()
        }
    );
    assert_eq!(entity.current_lease, Some(first.lease_id.clone()));
    assert_eq!(entity.persistence_tag.name.as_str(), "workspace");

    release(&core, &first.lease_id).await?;
    assert_eq!(entity_of(&store).await?.record.state, EntityState::Stopped);

    let second = granted(core.handle_lease_request(named("k2")?).await)?;
    assert_ne!(second.lease_id, first.lease_id);
    assert_eq!(second.entity_id, Some(id.clone()));
    let entity = entity_of(&store).await?.record;
    assert_eq!(entity.generation, 2);
    assert_eq!(entity.current_lease, Some(second.lease_id.clone()));

    let events = sink.events();
    assert!(matches!(events[0], EntityLifecycleEvent::Created { .. }));
    let started = events
        .iter()
        .filter(|e| matches!(e, EntityLifecycleEvent::RunStarted { .. }))
        .count();
    assert_eq!(started, 2);
    // The run-ended event follows once the head poll gives up.
    for _ in 0..300 {
        if sink
            .events()
            .iter()
            .any(|e| matches!(e, EntityLifecycleEvent::RunEnded { generation: 1, .. }))
        {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Err("no RunEnded for generation 1".into())
}

#[tokio::test]
async fn a_stale_release_does_not_stop_a_newer_run() -> TestResult {
    let store = Arc::new(InMemorySandboxEntityStore::new());
    let (core, _) = shadowed(make_core(mock_provider_response()?)?, store.clone());
    let first = granted(core.handle_lease_request(named("k1")?).await)?;
    let entity = entity_of(&store).await?;
    let other = LeaseId::new("someone-else");
    store
        .mutate(&TenantId::new("test-tenant")?, &entity.record.entity_id, &|e| {
            e.current_lease = Some(other.clone());
            e.state = EntityState::Running { lease: other.clone() };
            Ok(())
        })
        .await?;
    release(&core, &first.lease_id).await?;
    assert_eq!(
        entity_of(&store).await?.record.state,
        EntityState::Running { lease: other }
    );
    Ok(())
}

#[tokio::test]
async fn a_running_named_lease_is_adopted_on_reconnect() -> TestResult {
    let store = Arc::new(InMemorySandboxEntityStore::new());
    let plain = make_core(mock_provider_response()?)?;
    let first = granted(plain.handle_lease_request(named("k1")?).await)?;
    assert!(first.entity_id.is_none());
    assert!(
        store
            .get_by_name(&TenantId::new("test-tenant")?, "workspace")
            .await?
            .is_none()
    );

    let (core, _) = shadowed(plain, store.clone());
    let again = granted(core.handle_lease_request(named("k2")?).await)?;
    assert!(!again.created.is_created());
    assert_eq!(again.lease_id, first.lease_id);
    let entity = entity_of(&store).await?.record;
    assert_eq!(again.entity_id, Some(entity.entity_id.clone()));
    assert_eq!(entity.current_lease, Some(first.lease_id.clone()));
    assert_eq!(entity.state, EntityState::Running { lease: first.lease_id });
    Ok(())
}

#[tokio::test]
async fn a_tag_only_name_gets_an_entity_on_first_start() -> TestResult {
    let store = Arc::new(InMemorySandboxEntityStore::new());
    let (core, _) = shadowed(make_core(mock_provider_response()?)?, store.clone());
    let tag = snapshot_tag_of("test-tenant", "workspace")?;
    core.tag_store.set(&tag, tag_record("test")?, None).await?;
    let grant = granted(core.handle_lease_request(named("k1")?).await)?;
    let entity = entity_of(&store).await?.record;
    assert_eq!(grant.entity_id, Some(entity.entity_id));
    assert_eq!(entity.persistence_tag, tag);
    assert_eq!(entity.generation, 1);
    Ok(())
}

#[tokio::test]
async fn a_changed_tag_after_release_becomes_the_head() -> TestResult {
    let store = Arc::new(InMemorySandboxEntityStore::new());
    let (core, sink) = shadowed(make_core(mock_provider_response()?)?, store.clone());
    let grant = granted(core.handle_lease_request(named("k1")?).await)?;
    release(&core, &grant.lease_id).await?;
    let tag = snapshot_tag_of("test-tenant", "workspace")?;
    core.tag_store.set(&tag, tag_record("builder")?, None).await?;
    for _ in 0..200 {
        if entity_of(&store).await?.record.head.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let head = entity_of(&store).await?.record.head.ok_or("head")?;
    assert_eq!(head.digest, test_digest()?);
    assert_eq!(head.from_lease, grant.lease_id);
    for _ in 0..200 {
        if sink.events().iter().any(|e| {
            matches!(
                e,
                EntityLifecycleEvent::RunEnded {
                    captured_digest: Some(_),
                    ..
                }
            )
        }) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Err("no RunEnded with a captured digest".into())
}

fn published_with_tag(
    lease: &LeaseId,
    tag_set_at: Option<SystemTime>,
) -> anyhow::Result<trogon_sandbox_contract::SnapshotPublished> {
    Ok(trogon_sandbox_contract::SnapshotPublished {
        lease_id: lease.clone(),
        digest: test_digest()?,
        artifact_type: trogon_sandbox_contract::ArtifactType::SandboxRootfsV1,
        tenant: TenantId::new("test-tenant")?,
        bytes: 1,
        generation: None,
        correlation_id: None,
        tag: match tag_set_at {
            Some(set_at) => Some(trogon_sandbox_contract::SnapshotPublishedTag {
                name: SnapshotTagName::new("workspace")?,
                set_at,
                retention: None,
            }),
            None => None,
        },
    })
}

#[tokio::test]
async fn a_slow_capture_still_records_the_head_from_the_published_event() -> TestResult {
    let store = Arc::new(InMemorySandboxEntityStore::new());
    let sink = Arc::new(InMemoryEntityEventSink::new());
    // One immediate look at the tag and no more: the capture is slower.
    let shadow = EntityShadow::new(
        SandboxEntityMode::Shadow,
        store.clone(),
        sink.clone(),
        Duration::from_millis(10),
    )
    .with_head_poll(Duration::from_millis(1), 1);
    let core = make_core(mock_provider_response()?)?.with_entity_shadow(Arc::new(shadow));
    let grant = granted(core.handle_lease_request(named("k1")?).await)?;
    release(&core, &grant.lease_id).await?;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(entity_of(&store).await?.record.head.is_none());

    let set_at = SystemTime::now();
    core.entity_snapshot_published(&published_with_tag(&grant.lease_id, Some(set_at))?)
        .await;

    let head = entity_of(&store).await?.record.head.ok_or("head")?;
    assert_eq!(head.digest, test_digest()?);
    assert_eq!(head.from_lease, grant.lease_id);
    assert_eq!(head.captured_at, set_at);

    // An older event never overwrites a newer head, and an untagged one is ignored.
    let older = set_at - Duration::from_secs(60);
    core.entity_snapshot_published(&published_with_tag(&grant.lease_id, Some(older))?)
        .await;
    core.entity_snapshot_published(&published_with_tag(&grant.lease_id, None)?)
        .await;
    assert_eq!(entity_of(&store).await?.record.head.ok_or("head")?.captured_at, set_at);
    Ok(())
}

struct FailingStore;

fn down() -> EntityStoreError {
    EntityStoreError::Backend("entity store down".into())
}

#[async_trait::async_trait]
impl SandboxEntityStore for FailingStore {
    async fn create(&self, _: NewSandboxEntity) -> Result<Versioned, EntityStoreError> {
        Err(down())
    }
    async fn get(&self, _: &TenantId, _: &SandboxEntityId) -> Result<Option<Versioned>, EntityStoreError> {
        Err(down())
    }
    async fn get_by_name(&self, _: &TenantId, _: &str) -> Result<Option<Versioned>, EntityStoreError> {
        Err(down())
    }
    async fn list(&self, _: &TenantId) -> Result<Vec<Versioned>, EntityStoreError> {
        Err(down())
    }
    async fn list_all(&self) -> Result<Vec<Versioned>, EntityStoreError> {
        Err(down())
    }
    async fn remove_dangling_name_claims(&self) -> Result<usize, EntityStoreError> {
        Err(down())
    }
    async fn compare_and_swap(
        &self,
        _: &trogon_sandbox_contract::SandboxEntityRecord,
        _: u64,
    ) -> Result<u64, EntityStoreError> {
        Err(down())
    }
    async fn delete(&self, _: &TenantId, _: &SandboxEntityId) -> Result<(), EntityStoreError> {
        Err(down())
    }
}

#[tokio::test]
async fn entity_failures_do_not_change_lease_responses() -> TestResult {
    let plain = make_core(mock_provider_response()?)?;
    let expected = granted(plain.handle_lease_request(named("k1")?).await)?;

    let (core, _) = shadowed(make_core(mock_provider_response()?)?, Arc::new(FailingStore));
    let grant = granted(core.handle_lease_request(named("k1")?).await)?;
    assert_eq!(grant.entity_id, None);
    assert!(grant.created.is_created());
    assert_eq!(grant.provider, expected.provider);
    assert_eq!(grant.sandbox_id, expected.sandbox_id);

    let again = granted(core.handle_lease_request(named("k2")?).await)?;
    assert!(!again.created.is_created());
    assert_eq!(again.lease_id, grant.lease_id);
    release(&core, &grant.lease_id).await?;
    assert_eq!(core.sweep_entities().await, Default::default());
    Ok(())
}

#[tokio::test]
async fn repair_returns_an_orphaned_starting_entity_to_stopped() -> TestResult {
    let store = Arc::new(InMemorySandboxEntityStore::new());
    let (core, sink) = shadowed(make_core(mock_provider_response()?)?, store.clone());
    let tenant = TenantId::new("test-tenant")?;
    let created = store.create(new_entity("workspace")?).await?;
    let id = created.record.entity_id;
    let lease = LeaseId::new("never-granted");
    store.apply_event(&tenant, &id, &EntityEvent::Provisioned).await?;
    store
        .apply_event(&tenant, &id, &EntityEvent::StartRequested { lease })
        .await?;

    // The first pass only records when the entity was first seen.
    assert_eq!(core.sweep_entities().await.runs_ended, 0);
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(core.sweep_entities().await.runs_ended, 1);
    assert_eq!(
        store.get(&tenant, &id).await?.ok_or("record")?.record.state,
        EntityState::Stopped
    );
    assert!(sink.events().iter().any(|e| matches!(
        e,
        EntityLifecycleEvent::RunEnded { reason, .. } if reason == "repair"
    )));
    Ok(())
}

#[tokio::test]
async fn repair_leaves_a_live_run_alone() -> TestResult {
    let store = Arc::new(InMemorySandboxEntityStore::new());
    let (core, _) = shadowed(make_core(mock_provider_response()?)?, store.clone());
    let grant = granted(core.handle_lease_request(named("k1")?).await)?;
    core.sweep_entities().await;
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert_eq!(core.sweep_entities().await.runs_ended, 0);
    assert_eq!(
        entity_of(&store).await?.record.state,
        EntityState::Running { lease: grant.lease_id }
    );
    Ok(())
}

#[tokio::test]
async fn repair_removes_a_dangling_name_claim() -> TestResult {
    let store = Arc::new(InMemorySandboxEntityStore::new());
    let (core, _) = shadowed(make_core(mock_provider_response()?)?, store.clone());
    store.insert_dangling_name(&TenantId::new("test-tenant")?, "gone")?;
    assert_eq!(core.sweep_entities().await.claims_removed, 1);
    assert_eq!(core.sweep_entities().await.claims_removed, 0);
    Ok(())
}

#[tokio::test]
async fn deleting_a_named_sandbox_deletes_the_entity() -> TestResult {
    let store = Arc::new(InMemorySandboxEntityStore::new());
    let (core, sink) = shadowed(
        make_core_with_purge_named_response(PurgeNamedResponse::NotFound)?,
        store.clone(),
    );
    granted(core.handle_lease_request(named("k1")?).await)?;
    core.handle_delete_named_sandbox(DeleteNamedSandboxRequest {
        tenant: TenantId::new("test-tenant")?,
        name: LeaseName::new("workspace")?,
    })
    .await;
    assert!(
        store
            .get_by_name(&TenantId::new("test-tenant")?, "workspace")
            .await?
            .is_none()
    );
    assert!(
        sink.events()
            .iter()
            .any(|e| matches!(e, EntityLifecycleEvent::Deleted { .. }))
    );
    Ok(())
}

/// Delegates to an in-memory store until told to fail, so a test can
/// break the store after the entity exists.
struct FlakyStore {
    inner: InMemorySandboxEntityStore,
    failing: std::sync::atomic::AtomicBool,
}

impl FlakyStore {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: InMemorySandboxEntityStore::new(),
            failing: std::sync::atomic::AtomicBool::new(false),
        })
    }

    fn fail(&self, failing: bool) {
        self.failing.store(failing, std::sync::atomic::Ordering::SeqCst);
    }

    fn check(&self) -> Result<(), EntityStoreError> {
        match self.failing.load(std::sync::atomic::Ordering::SeqCst) {
            true => Err(EntityStoreError::Backend("store down".into())),
            false => Ok(()),
        }
    }
}

#[async_trait::async_trait]
impl SandboxEntityStore for FlakyStore {
    async fn create(&self, new: NewSandboxEntity) -> Result<Versioned, EntityStoreError> {
        self.check()?;
        self.inner.create(new).await
    }
    async fn get(&self, tenant: &TenantId, id: &SandboxEntityId) -> Result<Option<Versioned>, EntityStoreError> {
        self.check()?;
        self.inner.get(tenant, id).await
    }
    async fn get_by_name(&self, tenant: &TenantId, name: &str) -> Result<Option<Versioned>, EntityStoreError> {
        self.check()?;
        self.inner.get_by_name(tenant, name).await
    }
    async fn list(&self, tenant: &TenantId) -> Result<Vec<Versioned>, EntityStoreError> {
        self.check()?;
        self.inner.list(tenant).await
    }
    async fn list_all(&self) -> Result<Vec<Versioned>, EntityStoreError> {
        self.check()?;
        self.inner.list_all().await
    }
    async fn remove_dangling_name_claims(&self) -> Result<usize, EntityStoreError> {
        self.check()?;
        self.inner.remove_dangling_name_claims().await
    }
    async fn compare_and_swap(
        &self,
        record: &trogon_sandbox_contract::SandboxEntityRecord,
        revision: u64,
    ) -> Result<u64, EntityStoreError> {
        self.check()?;
        self.inner.compare_and_swap(record, revision).await
    }
    async fn delete(&self, tenant: &TenantId, id: &SandboxEntityId) -> Result<(), EntityStoreError> {
        self.check()?;
        self.inner.delete(tenant, id).await
    }
}

fn with_mode(core: BrokerCore, store: Arc<dyn SandboxEntityStore>, mode: SandboxEntityMode) -> BrokerCore {
    let shadow = EntityShadow::new(
        mode,
        store,
        Arc::new(InMemoryEntityEventSink::new()),
        Duration::from_millis(10),
    )
    .with_head_poll(Duration::from_millis(10), 5);
    core.with_entity_shadow(Arc::new(shadow))
}

fn tenant() -> anyhow::Result<TenantId> {
    Ok(TenantId::new("test-tenant")?)
}

#[tokio::test]
async fn on_mode_keeps_the_entity_id_stable_and_serves_it() -> TestResult {
    let store = Arc::new(InMemorySandboxEntityStore::new());
    let core = with_mode(
        make_core(mock_provider_response()?)?,
        store.clone(),
        SandboxEntityMode::On,
    );
    let first = granted(core.handle_lease_request(named("k1")?).await)?;
    let id = first.entity_id.clone().ok_or("entity id")?;
    release(&core, &first.lease_id).await?;
    let second = granted(core.handle_lease_request(named("k2")?).await)?;
    assert_eq!(second.entity_id, Some(id.clone()));

    for entity in [EntityRef::Id(id.clone()), EntityRef::Name("workspace".into())] {
        match core
            .handle_get_entity(GetEntityRequest {
                tenant: tenant()?,
                entity,
            })
            .await
        {
            GetEntityResponse::Found { row } => {
                assert_eq!(row.entity_id, id);
                assert_eq!(row.generation, 2);
                assert_eq!(row.current_lease, Some(second.lease_id.clone()));
            }
            other => return Err(format!("expected the entity, got {other:?}").into()),
        }
    }
    Ok(())
}

#[tokio::test]
async fn on_mode_fails_the_request_when_the_entity_store_fails() -> TestResult {
    let store = FlakyStore::new();
    let core = with_mode(
        make_core(mock_provider_response()?)?,
        store.clone(),
        SandboxEntityMode::On,
    );
    store.fail(true);
    match core.handle_lease_request(named("k1")?).await {
        LeaseResponse::Denied(LeaseError::EntityUnavailable { reason }) => {
            assert!(reason.contains("store down"), "{reason}")
        }
        other => return Err(format!("expected EntityUnavailable, got {other:?}").into()),
    }
    // A later attempt, once the store is back, is not poisoned.
    store.fail(false);
    granted(core.handle_lease_request(named("k1")?).await)?;
    Ok(())
}

#[tokio::test]
async fn shadow_mode_still_swallows_entity_store_failures() -> TestResult {
    let store = FlakyStore::new();
    let core = with_mode(
        make_core(mock_provider_response()?)?,
        store.clone(),
        SandboxEntityMode::Shadow,
    );
    store.fail(true);
    let grant = granted(core.handle_lease_request(named("k1")?).await)?;
    assert!(grant.entity_id.is_none());
    Ok(())
}

#[tokio::test]
async fn on_mode_resolves_a_name_through_the_entity() -> TestResult {
    let store = FlakyStore::new();
    let core = with_mode(
        make_core(mock_provider_response()?)?,
        store.clone(),
        SandboxEntityMode::On,
    );
    // Unknown to the entity and to the legacy index.
    assert_eq!(core.resolve_named_lease(&tenant()?, "workspace").await, Ok(None));
    let grant = granted(core.handle_lease_request(named("k1")?).await)?;
    assert_eq!(
        core.resolve_named_lease(&tenant()?, "workspace").await,
        Ok(Some(grant.lease_id.clone()))
    );
    release(&core, &grant.lease_id).await?;
    assert_eq!(core.resolve_named_lease(&tenant()?, "workspace").await, Ok(None));
    store.fail(true);
    assert!(matches!(
        core.resolve_named_lease(&tenant()?, "workspace").await,
        Err(LeaseError::EntityUnavailable { .. })
    ));
    match core
        .handle_get_lease_by_name(GetLeaseByNameRequest {
            tenant: tenant()?,
            name: LeaseName::new("workspace")?,
        })
        .await
    {
        GetLeaseByNameResponse::Denied(LeaseError::EntityUnavailable { .. }) => {}
        other => return Err(format!("expected EntityUnavailable, got {other:?}").into()),
    }
    Ok(())
}

#[tokio::test]
async fn off_and_shadow_resolve_names_from_the_legacy_index_only() -> TestResult {
    let off = make_core(mock_provider_response()?)?;
    let grant = granted(off.handle_lease_request(named("k1")?).await)?;
    assert_eq!(
        off.resolve_named_lease(&tenant()?, "workspace").await,
        Ok(Some(grant.lease_id))
    );

    let store = FlakyStore::new();
    let shadow = with_mode(
        make_core(mock_provider_response()?)?,
        store.clone(),
        SandboxEntityMode::Shadow,
    );
    let grant = granted(shadow.handle_lease_request(named("k1")?).await)?;
    store.fail(true);
    assert_eq!(
        shadow.resolve_named_lease(&tenant()?, "workspace").await,
        Ok(Some(grant.lease_id))
    );
    Ok(())
}

#[tokio::test]
async fn entity_verbs_report_not_enabled_when_off_and_delete_needs_on() -> TestResult {
    let off = make_core(mock_provider_response()?)?;
    let by_name = EntityRef::Name("workspace".into());
    assert!(matches!(
        off.handle_get_entity(GetEntityRequest {
            tenant: tenant()?,
            entity: by_name.clone()
        })
        .await,
        GetEntityResponse::Denied(LeaseError::EntityNotEnabled)
    ));
    assert!(matches!(
        off.handle_list_entities(ListEntitiesRequest {
            tenant: tenant()?,
            limit: None,
            cursor: None
        })
        .await,
        ListEntitiesResponse::Denied(LeaseError::EntityNotEnabled)
    ));
    assert!(matches!(
        off.handle_delete_entity(DeleteEntityRequest {
            tenant: tenant()?,
            entity: by_name.clone()
        })
        .await,
        DeleteEntityResponse::Denied(LeaseError::EntityNotEnabled)
    ));

    let shadow = with_mode(
        make_core(mock_provider_response()?)?,
        Arc::new(InMemorySandboxEntityStore::new()),
        SandboxEntityMode::Shadow,
    );
    granted(shadow.handle_lease_request(named("k1")?).await)?;
    assert!(matches!(
        shadow
            .handle_get_entity(GetEntityRequest {
                tenant: tenant()?,
                entity: by_name.clone()
            })
            .await,
        GetEntityResponse::Found { .. }
    ));
    assert!(matches!(
        shadow
            .handle_delete_entity(DeleteEntityRequest {
                tenant: tenant()?,
                entity: by_name
            })
            .await,
        DeleteEntityResponse::Denied(LeaseError::EntityNotEnabled)
    ));
    Ok(())
}

#[tokio::test]
async fn on_mode_deletes_an_entity_by_name_and_by_id() -> TestResult {
    let store = Arc::new(InMemorySandboxEntityStore::new());
    let core = with_mode(
        make_core_with_purge_named_response(PurgeNamedResponse::NotFound)?,
        store.clone(),
        SandboxEntityMode::On,
    );
    let grant = granted(core.handle_lease_request(named("k1")?).await)?;
    let id = grant.entity_id.ok_or("entity id")?;
    let owner = tenant()?;
    let delete = |entity| DeleteEntityRequest {
        tenant: owner.clone(),
        entity,
    };

    // Another tenant cannot see or delete it.
    let stranger = TenantId::new("someone-else")?;
    assert_eq!(
        core.handle_delete_entity(DeleteEntityRequest {
            tenant: stranger.clone(),
            entity: EntityRef::Id(id.clone()),
        })
        .await,
        DeleteEntityResponse::NotFound
    );
    assert_eq!(
        core.handle_get_entity(GetEntityRequest {
            tenant: stranger,
            entity: EntityRef::Id(id.clone()),
        })
        .await,
        GetEntityResponse::NotFound
    );

    assert_eq!(
        core.handle_delete_entity(delete(EntityRef::Id(id.clone()))).await,
        DeleteEntityResponse::Deleted
    );
    assert!(store.get(&tenant()?, &id).await?.is_none());
    assert_eq!(
        core.handle_delete_entity(delete(EntityRef::Id(id))).await,
        DeleteEntityResponse::NotFound
    );

    let grant = granted(core.handle_lease_request(named("k2")?).await)?;
    assert!(grant.created.is_created());
    assert_eq!(
        core.handle_delete_entity(delete(EntityRef::Name("workspace".into())))
            .await,
        DeleteEntityResponse::Deleted
    );
    assert!(store.get_by_name(&tenant()?, "workspace").await?.is_none());
    Ok(())
}

#[tokio::test]
async fn on_mode_deletes_an_unnamed_stopped_entity() -> TestResult {
    let store = Arc::new(InMemorySandboxEntityStore::new());
    let core = with_mode(
        make_core(mock_provider_response()?)?,
        store.clone(),
        SandboxEntityMode::On,
    );
    let mut new = new_entity("unused")?;
    new.name = None;
    let id = store.create(new).await?.record.entity_id;
    assert_eq!(
        core.handle_delete_entity(DeleteEntityRequest {
            tenant: tenant()?,
            entity: EntityRef::Id(id.clone()),
        })
        .await,
        DeleteEntityResponse::Deleted
    );
    assert!(store.get(&tenant()?, &id).await?.is_none());
    Ok(())
}

#[tokio::test]
async fn list_entities_pages_by_id_and_stays_inside_the_tenant() -> TestResult {
    let store = Arc::new(InMemorySandboxEntityStore::new());
    let core = with_mode(
        make_core(mock_provider_response()?)?,
        store.clone(),
        SandboxEntityMode::On,
    );
    let mut ids = Vec::new();
    for name in ["a", "b", "c"] {
        ids.push(store.create(new_entity(name)?).await?.record.entity_id);
    }
    let mut foreign = new_entity("z")?;
    foreign.tenant = TenantId::new("someone-else")?;
    store.create(foreign).await?;
    ids.sort();

    let mut seen = Vec::new();
    let mut cursor = None;
    let mut pages = 0;
    loop {
        let ListEntitiesResponse::Listed { rows, next_cursor } = core
            .handle_list_entities(ListEntitiesRequest {
                tenant: tenant()?,
                limit: Some(2),
                cursor: cursor.clone(),
            })
            .await
        else {
            return Err("list denied".into());
        };
        pages += 1;
        assert!(rows.len() <= 2);
        seen.extend(rows.into_iter().map(|r| r.entity_id));
        match next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }
    assert_eq!(pages, 2);
    assert_eq!(seen, ids);
    Ok(())
}

#[tokio::test]
async fn entity_reads_surface_a_store_failure_as_a_typed_error() -> TestResult {
    let store = FlakyStore::new();
    let core = with_mode(
        make_core(mock_provider_response()?)?,
        store.clone(),
        SandboxEntityMode::On,
    );
    store.fail(true);
    assert!(matches!(
        core.handle_get_entity(GetEntityRequest {
            tenant: tenant()?,
            entity: EntityRef::Name("workspace".into())
        })
        .await,
        GetEntityResponse::Denied(LeaseError::EntityUnavailable { .. })
    ));
    assert!(matches!(
        core.handle_list_entities(ListEntitiesRequest {
            tenant: tenant()?,
            limit: None,
            cursor: None
        })
        .await,
        ListEntitiesResponse::Denied(LeaseError::EntityUnavailable { .. })
    ));
    Ok(())
}

#[cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    expect(test_module_naming, reason = "the test module is named for the behavior it covers")
)]
mod nats_recovery;
