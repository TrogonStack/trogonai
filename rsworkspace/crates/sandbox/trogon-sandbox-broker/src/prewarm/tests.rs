use super::*;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
use trogon_sandbox_contract::ProviderKind;

#[test]
fn lru_budget_keeps_popular_digests() -> TestResult {
    let d1 = Digest::new("sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")?;
    let d2 = Digest::new("sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb")?;
    let entries = vec![
        (
            d1.clone(),
            PrewarmState {
                tenant: TenantId::new("t")?,
                digest: d1.clone(),
                spec: PrewarmSpec {
                    pool_size: 2,
                    providers: vec![ProviderKind::LocalDocker],
                },
                pending_dispatches: 0,
                popularity: 1,
                updated_at: SystemTime::now(),
            },
        ),
        (
            d2.clone(),
            PrewarmState {
                tenant: TenantId::new("t")?,
                digest: d2.clone(),
                spec: PrewarmSpec {
                    pool_size: 2,
                    providers: vec![ProviderKind::LocalDocker],
                },
                pending_dispatches: 0,
                popularity: 10,
                updated_at: SystemTime::now(),
            },
        ),
    ];
    let kept = lru_budget_filter(&entries, 2);
    assert_eq!(kept.len(), 1);
    assert_eq!(kept[0].0, d2);
    Ok(())
}

#[tokio::test]
async fn in_memory_store_lists_configured_policy() -> TestResult {
    let store = InMemoryPrewarmStore::new();
    let tenant = TenantId::new("prewarm-tenant")?;
    let digest = Digest::new("sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa")?;
    PrewarmStore::put(
        store.as_ref(),
        &tenant,
        &digest,
        &PrewarmState::new(
            tenant.clone(),
            digest.clone(),
            PrewarmSpec {
                pool_size: 2,
                providers: vec![ProviderKind::LocalDocker],
            },
        ),
    )
    .await?;
    let listed = store.list_all().await?;
    assert_eq!(listed.len(), 1);
    Ok(())
}

fn prewarm_state(tenant: &TenantId, digest: &Digest, pool_size: u32) -> PrewarmState {
    PrewarmState::new(
        tenant.clone(),
        digest.clone(),
        PrewarmSpec {
            pool_size,
            providers: vec![ProviderKind::LocalDocker],
        },
    )
}

fn prewarm_digest() -> anyhow::Result<Digest> {
    Ok(Digest::new(
        "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc",
    )?)
}

async fn nats_prewarm_store() -> anyhow::Result<(crate::test_support::LocalNats, NatsKvPrewarmStore)> {
    let server = crate::test_support::LocalNats::start().await;
    let store = NatsKvPrewarmStore::new(jetstream::new(server.client.clone())).await?;
    Ok((server, store))
}

#[tokio::test]
async fn nats_tenants_sharing_a_legacy_key_keep_independent_prewarm_records() -> TestResult {
    let (_server, store) = nats_prewarm_store().await?;
    let digest = prewarm_digest()?;
    let dotted = TenantId::new("a.b")?;
    let underscored = TenantId::new("a_b")?;

    store.put(&dotted, &digest, &prewarm_state(&dotted, &digest, 1)).await?;
    store
        .put(&underscored, &digest, &prewarm_state(&underscored, &digest, 2))
        .await?;

    let dotted_state = store.get(&dotted, &digest).await?.ok_or("prewarm record")?;
    assert_eq!(dotted_state.tenant, dotted);
    assert_eq!(dotted_state.spec.pool_size, 1);
    let listed = store.list_for_tenant(&underscored).await?;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].1.spec.pool_size, 2);

    store.delete(&dotted, &digest).await?;
    assert!(store.get(&dotted, &digest).await?.is_none());
    assert!(store.get(&underscored, &digest).await?.is_some());
    assert_eq!(store.list_all().await?.len(), 1);
    Ok(())
}

#[tokio::test]
async fn nats_legacy_prewarm_record_is_read_only_by_its_tenant_and_moved_on_put() -> TestResult {
    let (_server, store) = nats_prewarm_store().await?;
    let digest = prewarm_digest()?;
    let owner = TenantId::new("a.b")?;
    let other = TenantId::new("a:b")?;
    let legacy = legacy_prewarm_kv_key(&owner, &digest);
    assert_eq!(legacy, legacy_prewarm_kv_key(&other, &digest));
    store
        .kv
        .put(
            &legacy,
            Bytes::from(serde_json::to_vec(&prewarm_state(&owner, &digest, 3))?),
        )
        .await?;

    assert_eq!(
        store
            .get(&owner, &digest)
            .await?
            .ok_or("prewarm record")?
            .spec
            .pool_size,
        3
    );
    assert!(store.get(&other, &digest).await?.is_none());
    assert!(store.list_for_tenant(&other).await?.is_empty());
    assert_eq!(store.list_for_tenant(&owner).await?.len(), 1);

    store.put(&owner, &digest, &prewarm_state(&owner, &digest, 4)).await?;
    assert!(store.kv.get(&legacy).await?.is_none());
    assert_eq!(
        store
            .get(&owner, &digest)
            .await?
            .ok_or("prewarm record")?
            .spec
            .pool_size,
        4
    );
    assert_eq!(store.list_all().await?.len(), 1);
    Ok(())
}
