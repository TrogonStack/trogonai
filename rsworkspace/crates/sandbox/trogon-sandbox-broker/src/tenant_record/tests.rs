use std::error::Error;

use async_nats::jetstream;

use super::*;
type TestResult = Result<(), Box<dyn Error>>;

use crate::image_policy::{ImageAllowList, NatsKvImageAllowList, TenantImagePolicy};

fn tenant(name: &str) -> Result<TenantId, Box<dyn Error>> {
    Ok(TenantId::new(name)?)
}

fn policy(pattern: &str) -> Result<Bytes, Box<dyn Error>> {
    let policy = TenantImagePolicy::from_patterns(vec![pattern.to_string()]);
    Ok(Bytes::from(serde_json::to_vec(&policy)?))
}

async fn patterns(store: &NatsKvImageAllowList, tenant: &TenantId) -> Result<Option<Vec<String>>, Box<dyn Error>> {
    Ok(store.allowed_for(tenant).await?.map(|policy| policy.patterns))
}

#[tokio::test]
async fn tenants_sharing_a_legacy_key_keep_independent_records() -> TestResult {
    let server = crate::test_support::LocalNats::start().await;
    let store = NatsKvImageAllowList::new(jetstream::new(server.client.clone()), "policy").await?;
    let dotted = tenant("a.b")?;
    let underscored = tenant("a_b")?;

    put(&store.kv, &dotted, policy("dotted/*")?).await?;
    put(&store.kv, &underscored, policy("underscored/*")?).await?;

    assert_eq!(patterns(&store, &dotted).await?, Some(vec!["dotted/*".to_string()]));
    assert_eq!(
        patterns(&store, &underscored).await?,
        Some(vec!["underscored/*".to_string()])
    );

    assert!(clear(&store.kv, &dotted).await?);
    assert_eq!(patterns(&store, &dotted).await?, None);
    assert_eq!(
        patterns(&store, &underscored).await?,
        Some(vec!["underscored/*".to_string()])
    );
    Ok(())
}

#[tokio::test]
async fn legacy_record_is_read_until_the_tenant_gets_a_current_one() -> TestResult {
    let server = crate::test_support::LocalNats::start().await;
    let store = NatsKvImageAllowList::new(jetstream::new(server.client.clone()), "policy").await?;
    let tenant = tenant("org.team")?;
    store.kv.put(legacy_tenant_kv_key(&tenant), policy("legacy/*")?).await?;

    assert_eq!(patterns(&store, &tenant).await?, Some(vec!["legacy/*".to_string()]));

    put(&store.kv, &tenant, policy("current/*")?).await?;
    assert_eq!(patterns(&store, &tenant).await?, Some(vec!["current/*".to_string()]));

    assert!(clear(&store.kv, &tenant).await?);
    assert_eq!(patterns(&store, &tenant).await?, None);
    assert!(!clear(&store.kv, &tenant).await?);
    Ok(())
}
