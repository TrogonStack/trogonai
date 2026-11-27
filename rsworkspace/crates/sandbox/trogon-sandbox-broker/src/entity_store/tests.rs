use super::*;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
use std::collections::BTreeMap;
use std::num::NonZeroU32;
use std::sync::Arc;
use trogon_sandbox_contract::LeaseId;
use trogon_sandbox_contract::{OciImageRef, SandboxImage, SandboxSpec, SandboxSpecParts, SandboxTags, WorkspaceSource};

fn tenant(name: &str) -> anyhow::Result<TenantId> {
    Ok(TenantId::new(name)?)
}

fn new_entity(tenant_name: &str, name: Option<&str>) -> anyhow::Result<NewSandboxEntity> {
    Ok(NewSandboxEntity {
        tenant: tenant(tenant_name)?,
        name: name.map(str::to_string),
        created_by: None,
        spec: SandboxSpec::new(SandboxSpecParts {
            image: SandboxImage::Registry {
                reference: OciImageRef::parse_unqualified("ubuntu:24.04"),
                pull_secret: None,
            },
            workspace: WorkspaceSource::empty(),
            env: BTreeMap::new(),
            cpu_seconds: NonZeroU32::new(60).ok_or_else(|| anyhow::anyhow!("zero cpu"))?,
            memory_mib: NonZeroU32::new(512).ok_or_else(|| anyhow::anyhow!("zero memory"))?,
            snapshot_out: None,
            network_policy: None,
            mounts: Vec::new(),
            drives: Vec::new(),
            exposed_ports: Vec::new(),
            boot_mode: Default::default(),
            working_dir: None,
            user: None,
            privileged: Default::default(),
            wasi_capabilities: None,
            fs_policy: None,
            secret_bindings: Default::default(),
            platform: Default::default(),
            idle_timeout_seconds: None,
            no_ttl: Default::default(),
            idle_policy: None,
            vcpu_millis: None,
            disk_mib: None,
            retention: Default::default(),
        })?,
        metadata: SandboxTags::default(),
        drives: vec![],
        persistence_tag: None,
    })
}

async fn crud_and_cas(store: &dyn SandboxEntityStore) -> TestResult {
    let t = tenant("acme")?;
    let created = store.create(new_entity("acme", Some("dev"))?).await?;
    let id = created.record.entity_id.clone();
    assert_eq!(created.record.state, EntityState::Creating);
    assert_eq!(created.record.persistence_tag.name.as_str(), format!("e-{id}"));

    let got = store.get(&t, &id).await?.ok_or("entity")?;
    assert_eq!(got, created);
    let by_name = store.get_by_name(&t, "dev").await?.ok_or("entity")?;
    assert_eq!(by_name.record.entity_id, id);

    let provisioned = store.apply_event(&t, &id, &EntityEvent::Provisioned).await?;
    assert_eq!(provisioned.record.state, EntityState::Stopped);
    assert!(provisioned.revision > created.revision);

    // a stale revision loses
    let mut stale = created.record.clone();
    stale.generation = 99;
    assert!(matches!(
        store.compare_and_swap(&stale, created.revision).await,
        Err(EntityStoreError::RevisionConflict)
    ));
    assert_eq!(store.get(&t, &id).await?.ok_or("entity")?.record.generation, 0);

    // identity is immutable
    let mut renamed = provisioned.record.clone();
    renamed.name = Some("other".to_string());
    assert!(matches!(
        store.compare_and_swap(&renamed, provisioned.revision).await,
        Err(EntityStoreError::InvalidArgument(_))
    ));

    // illegal transition surfaces and writes nothing
    let err = store
        .apply_event(&t, &id, &EntityEvent::Recovered)
        .await
        .err()
        .ok_or("expected a rejected transition")?;
    assert!(matches!(err, EntityStoreError::IllegalTransition(_)));

    let started = store
        .apply_event(
            &t,
            &id,
            &EntityEvent::StartRequested {
                lease: LeaseId::new("l1"),
            },
        )
        .await?;
    assert_eq!(started.record.generation, 1);
    assert_eq!(started.record.current_lease, Some(LeaseId::new("l1")));

    assert_eq!(store.list(&t).await?.len(), 1);
    store.delete(&t, &id).await?;
    assert!(store.get(&t, &id).await?.is_none());
    assert!(store.get_by_name(&t, "dev").await?.is_none());
    assert!(matches!(store.delete(&t, &id).await, Err(EntityStoreError::NotFound)));
    // the name is free again
    store.create(new_entity("acme", Some("dev"))?).await?;
    Ok(())
}

async fn concurrent_mutations_all_apply(store: Arc<dyn SandboxEntityStore>) -> TestResult {
    let t = tenant("acme")?;
    let id = store.create(new_entity("acme", None)?).await?.record.entity_id;
    let mut tasks = Vec::new();
    for _ in 0..4 {
        let store = store.clone();
        let (t, id) = (t.clone(), id.clone());
        tasks.push(tokio::spawn(async move {
            store
                .mutate(&t, &id, &|r| {
                    r.generation += 1;
                    Ok(())
                })
                .await
        }));
    }
    for task in tasks {
        task.await??;
    }
    assert_eq!(store.get(&t, &id).await?.ok_or("entity")?.record.generation, 4);
    Ok(())
}

async fn name_claim_race_has_one_winner(store: Arc<dyn SandboxEntityStore>) -> TestResult {
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let store = store.clone();
        let entity = new_entity("acme", Some("race"))?;
        tasks.push(tokio::spawn(async move { store.create(entity).await }));
    }
    let mut won = 0;
    for task in tasks {
        match task.await? {
            Ok(_) => won += 1,
            Err(EntityStoreError::NameTaken) => {}
            Err(other) => return Err(format!("unexpected {other:?}").into()),
        }
    }
    assert_eq!(won, 1);
    let all = store.list(&tenant("acme")?).await?;
    assert_eq!(all.len(), 1, "losers must leave no entity behind");
    Ok(())
}

async fn tenants_are_isolated(store: &dyn SandboxEntityStore) -> TestResult {
    let a = store.create(new_entity("acme", Some("same"))?).await?;
    let b = store.create(new_entity("acme_team", Some("same"))?).await?;
    let c = store.create(new_entity("other", Some("same"))?).await?;
    for (tenant_name, own) in [("acme", &a), ("acme_team", &b), ("other", &c)] {
        let t = tenant(tenant_name)?;
        let listed = store.list(&t).await?;
        assert_eq!(listed.len(), 1, "{tenant_name}");
        assert_eq!(listed[0].record.entity_id, own.record.entity_id);
        assert_eq!(
            store.get_by_name(&t, "same").await?.ok_or("entity")?.record.entity_id,
            own.record.entity_id
        );
    }
    // an id from another tenant is not visible, and cannot be deleted
    assert!(store.get(&tenant("acme_team")?, &a.record.entity_id).await?.is_none());
    assert!(matches!(
        store.delete(&tenant("acme_team")?, &a.record.entity_id).await,
        Err(EntityStoreError::NotFound)
    ));
    assert!(store.get(&tenant("acme")?, &a.record.entity_id).await?.is_some());
    Ok(())
}

async fn rejects_empty_names(store: &dyn SandboxEntityStore) -> TestResult {
    assert!(matches!(
        store.create(new_entity("acme", Some(""))?).await,
        Err(EntityStoreError::InvalidArgument(_))
    ));
    // an empty tenant is rejected when the id is parsed, before it can reach a store
    assert!(TenantId::new("").is_err());
    Ok(())
}

#[test]
fn keys_are_injective_and_prefix_exact() -> TestResult {
    let id = SandboxEntityId::new("01ARZ3NDEKTSV4RRFFQ69G5FAV")?;
    let acme = entity_kv_key(&tenant("acme")?, &id);
    let team = entity_kv_key(&tenant("acme_team")?, &id);
    assert_ne!(acme, team);
    assert!(!team.starts_with(&entity_tenant_prefix(&tenant("acme")?)));
    assert!(acme.starts_with(&entity_tenant_prefix(&tenant("acme")?)));
    // `acme` + `_team` style splits and dotted names cannot collide either
    assert_ne!(
        entity_name_kv_key(&tenant("acme_team")?, "data"),
        entity_name_kv_key(&tenant("acme")?, "team_data")
    );
    assert_ne!(
        entity_name_kv_key(&tenant("a.b")?, "c"),
        entity_name_kv_key(&tenant("a")?, "b.c")
    );
    assert!(acme.chars().all(|c| c.is_ascii_alphanumeric() || c == '.'));
    Ok(())
}

#[test]
fn mode_defaults_off_and_parses() {
    assert_eq!(SandboxEntityMode::default(), SandboxEntityMode::Off);
    assert_eq!("shadow".parse(), Ok(SandboxEntityMode::Shadow));
    assert_eq!("on".parse(), Ok(SandboxEntityMode::On));
    assert!("maybe".parse::<SandboxEntityMode>().is_err());
}

#[tokio::test]
async fn memory_crud_and_cas() -> TestResult {
    crud_and_cas(&InMemorySandboxEntityStore::new()).await?;
    Ok(())
}

#[tokio::test]
async fn memory_concurrent_mutations() -> TestResult {
    concurrent_mutations_all_apply(Arc::new(InMemorySandboxEntityStore::new())).await?;
    Ok(())
}

#[tokio::test]
async fn memory_name_claim_race() -> TestResult {
    name_claim_race_has_one_winner(Arc::new(InMemorySandboxEntityStore::new())).await?;
    Ok(())
}

#[tokio::test]
async fn memory_tenant_isolation() -> TestResult {
    tenants_are_isolated(&InMemorySandboxEntityStore::new()).await?;
    Ok(())
}

#[tokio::test]
async fn memory_rejects_invalid_input() -> TestResult {
    rejects_empty_names(&InMemorySandboxEntityStore::new()).await?;
    Ok(())
}

async fn list_all_spans_tenants(store: &dyn SandboxEntityStore) -> TestResult {
    store.create(new_entity("acme", Some("a"))?).await?;
    store.create(new_entity("acme_team", None)?).await?;
    assert_eq!(store.list_all().await?.len(), 2);
    assert_eq!(store.remove_dangling_name_claims().await?, 0);
    Ok(())
}

#[tokio::test]
async fn memory_list_all_and_dangling_claims() -> TestResult {
    let store = InMemorySandboxEntityStore::new();
    list_all_spans_tenants(&store).await?;
    store.insert_dangling_name(&tenant("acme")?, "ghost")?;
    assert_eq!(store.remove_dangling_name_claims().await?, 1);
    assert!(store.get_by_name(&tenant("acme")?, "a").await?.is_some());
    Ok(())
}

#[tokio::test]
async fn nats_list_all_and_dangling_claims() -> TestResult {
    let (server, store) = nats_store().await?;
    list_all_spans_tenants(&store).await?;
    nats_names(&server)
        .await?
        .create(
            entity_name_kv_key(&tenant("acme")?, "ghost"),
            Bytes::from(new_entity_id()?.to_string()),
        )
        .await?;
    assert_eq!(store.remove_dangling_name_claims().await?, 1);
    assert!(store.get_by_name(&tenant("acme")?, "a").await?.is_some());
    assert_eq!(store.remove_dangling_name_claims().await?, 0);
    Ok(())
}

async fn nats_names(server: &crate::test_support::LocalNats) -> anyhow::Result<kv::Store> {
    Ok(jetstream::new(server.client.clone())
        .get_key_value(ENTITY_NAME_INDEX_KV_BUCKET)
        .await?)
}

async fn nats_store() -> anyhow::Result<(crate::test_support::LocalNats, NatsKvSandboxEntityStore)> {
    let server = crate::test_support::LocalNats::start().await;
    let store = NatsKvSandboxEntityStore::new(jetstream::new(server.client.clone())).await?;
    Ok((server, store))
}

#[tokio::test]
async fn nats_crud_and_cas() -> TestResult {
    let (_server, store) = nats_store().await?;
    crud_and_cas(&store).await?;
    Ok(())
}

#[tokio::test]
async fn nats_concurrent_mutations() -> TestResult {
    let (_server, store) = nats_store().await?;
    concurrent_mutations_all_apply(Arc::new(store)).await?;
    Ok(())
}

#[tokio::test]
async fn nats_name_claim_race() -> TestResult {
    let (_server, store) = nats_store().await?;
    name_claim_race_has_one_winner(Arc::new(store)).await?;
    Ok(())
}

#[tokio::test]
async fn nats_tenant_isolation() -> TestResult {
    let (_server, store) = nats_store().await?;
    tenants_are_isolated(&store).await?;
    Ok(())
}

#[tokio::test]
async fn nats_rejects_invalid_input() -> TestResult {
    let (_server, store) = nats_store().await?;
    rejects_empty_names(&store).await?;
    Ok(())
}

#[tokio::test]
async fn nats_dangling_name_claim_is_taken_over() -> TestResult {
    let (server, store) = nats_store().await?;
    let t = tenant("acme")?;
    let ghost = new_entity_id()?;
    nats_names(&server)
        .await?
        .create(entity_name_kv_key(&t, "ghost"), Bytes::from(ghost.to_string()))
        .await?;
    assert!(store.get_by_name(&t, "ghost").await?.is_none());
    let created = store.create(new_entity("acme", Some("ghost"))?).await?;
    let found = store.get_by_name(&t, "ghost").await?.ok_or("entity")?;
    assert_eq!(found.record.entity_id, created.record.entity_id);
    Ok(())
}
