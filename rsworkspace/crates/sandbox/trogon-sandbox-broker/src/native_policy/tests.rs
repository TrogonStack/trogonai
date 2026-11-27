use std::sync::Arc;

use async_nats::jetstream;
use trogon_sandbox_contract::TenantId;

use std::error::Error;

use super::*;

type TestResult = Result<(), Box<dyn Error>>;

fn tenant(name: &str) -> Result<TenantId, Box<dyn Error>> {
    Ok(TenantId::new(name)?)
}

#[tokio::test]
async fn deny_is_the_default_posture() -> TestResult {
    assert!(!DenyNativePassthrough.allows(&tenant("acme")?).await?);
    Ok(())
}

#[tokio::test]
async fn allow_list_admits_only_listed_tenants() -> TestResult {
    let policy = TenantAllowListNativePassthrough::new([tenant("acme")?]);
    assert!(policy.allows(&tenant("acme")?).await?);
    assert!(!policy.allows(&tenant("other")?).await?);
    Ok(())
}

#[tokio::test]
async fn stored_record_allows_only_while_it_says_allowed() -> TestResult {
    let store = InMemoryNativePassthroughStore::new();
    let acme = tenant("acme")?;
    assert!(!store.allows(&acme).await?);
    store
        .insert(&acme, TenantNativePassthroughPolicy { allowed: true })
        .await;
    assert!(store.allows(&acme).await?);
    store
        .insert(&acme, TenantNativePassthroughPolicy { allowed: false })
        .await;
    assert!(!store.allows(&acme).await?);
    Ok(())
}

#[tokio::test]
async fn static_list_wins_over_a_stored_deny() -> TestResult {
    let store = Arc::new(InMemoryNativePassthroughStore::new());
    let acme = tenant("acme")?;
    let globex = tenant("globex")?;
    store
        .insert(&acme, TenantNativePassthroughPolicy { allowed: false })
        .await;
    store
        .insert(&globex, TenantNativePassthroughPolicy { allowed: true })
        .await;
    let policy =
        StaticThenStoredNativePassthrough::new([acme.clone()], Arc::clone(&store) as Arc<dyn NativePassthroughPolicy>);
    assert!(policy.allows(&acme).await?);
    assert!(policy.allows(&globex).await?);
    assert!(!policy.allows(&tenant("other")?).await?);
    Ok(())
}

#[tokio::test]
async fn nats_store_keeps_tenants_sharing_a_legacy_key_apart() -> TestResult {
    let server = crate::test_support::LocalNats::start().await;
    let store = NatsKvNativePassthroughStore::new(jetstream::new(server.client.clone()), "native").await?;
    let dotted = tenant("a.b")?;
    let underscored = tenant("a_b")?;
    let allowed = bytes::Bytes::from(serde_json::to_vec(&TenantNativePassthroughPolicy { allowed: true })?);

    assert!(!store.allows(&dotted).await?);
    crate::tenant_record::put(&store.kv, &dotted, allowed).await?;
    assert!(store.allows(&dotted).await?);
    assert!(!store.allows(&underscored).await?);

    assert!(crate::tenant_record::clear(&store.kv, &dotted).await?);
    assert!(!store.allows(&dotted).await?);
    Ok(())
}
