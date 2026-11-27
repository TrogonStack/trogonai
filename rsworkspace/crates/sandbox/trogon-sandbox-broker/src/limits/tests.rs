use std::error::Error;

use async_nats::jetstream;
use trogon_sandbox_contract::TenantId;

use super::*;
use crate::test_support::LocalNats;

type TestResult = Result<(), Box<dyn Error>>;

fn limit(cpu: u32) -> TenantLimit {
    TenantLimit {
        max_cpu_seconds_per_request: cpu,
        max_memory_mib_per_request: 512,
        per_tag_quota: Some(PerTagQuota {
            tag_key: "user".to_string(),
            max_active_leases: 2,
        }),
    }
}

#[tokio::test]
async fn in_memory_limits_return_what_was_inserted() -> TestResult {
    let limits = InMemoryTenantLimits::new();
    let tenant = TenantId::new("acme")?;
    assert!(limits.for_tenant(&tenant).await?.is_none());
    limits.insert(&tenant, limit(60)).await;
    let stored = limits.for_tenant(&tenant).await?.ok_or("limit not stored")?;
    assert_eq!(stored.max_cpu_seconds_per_request, 60);
    assert_eq!(
        stored.per_tag_quota,
        Some(PerTagQuota {
            tag_key: "user".to_string(),
            max_active_leases: 2
        })
    );
    Ok(())
}

#[tokio::test]
async fn nats_limits_read_the_tenant_record() -> TestResult {
    let server = LocalNats::start().await;
    let limits = NatsKvTenantLimits::new(jetstream::new(server.client.clone()), "limits").await?;
    let tenant = TenantId::new("acme")?;
    assert!(limits.for_tenant(&tenant).await?.is_none());
    crate::tenant_record::put(&limits.kv, &tenant, bytes::Bytes::from(serde_json::to_vec(&limit(90))?)).await?;
    let stored = limits.for_tenant(&tenant).await?.ok_or("limit not stored")?;
    assert_eq!(stored.max_cpu_seconds_per_request, 90);
    Ok(())
}

#[tokio::test]
async fn a_malformed_record_is_an_error() -> TestResult {
    let server = LocalNats::start().await;
    let limits = NatsKvTenantLimits::new(jetstream::new(server.client.clone()), "limits").await?;
    let tenant = TenantId::new("acme")?;
    crate::tenant_record::put(&limits.kv, &tenant, bytes::Bytes::from_static(b"{")).await?;
    assert!(limits.for_tenant(&tenant).await.is_err());
    Ok(())
}
