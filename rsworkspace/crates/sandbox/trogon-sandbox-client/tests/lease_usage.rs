mod common;
mod lease_support;

use common::{TestResult, client_replying, lease_id, tenant};
use lease_support::sample_usage;
use trogon_sandbox_client::ClientError;
use trogon_sandbox_contract::{
    AccountLimits, GetAccountLimitsResponse, GetLeaseUsageResponse, GetTenantUsageResponse, LeaseError, TenantUsage,
};

#[tokio::test]
async fn usage_returns_the_counters() -> TestResult {
    let expected = sample_usage()?;
    let response = GetLeaseUsageResponse::Usage {
        usage: expected.clone(),
    };
    let (client, log) = client_replying(&response)?;
    assert_eq!(client.usage(tenant()?, &lease_id()).await?, expected);
    assert_eq!(log.only()?.subject, "sandbox.lease.usage.lease-1");
    Ok(())
}

#[tokio::test]
async fn usage_returns_denied_for_an_unknown_lease() -> TestResult {
    let response = GetLeaseUsageResponse::Denied(LeaseError::LeaseNotFound { lease_id: lease_id() });
    let (client, _) = client_replying(&response)?;
    let result = client.usage(tenant()?, &lease_id()).await;
    assert!(matches!(
        result,
        Err(ClientError::Denied(LeaseError::LeaseNotFound { .. }))
    ));
    Ok(())
}

#[tokio::test]
async fn limits_returns_the_account_limits_and_surfaces_denial() -> TestResult {
    let limits = AccountLimits {
        active_leases: 2,
        max_active_leases: 50,
        cpu_seconds_used: 10,
        cpu_seconds_cap: 3600,
        wall_seconds_used: 20,
        wall_seconds_cap: 7200,
        max_cpu_seconds_per_request: 600,
        max_memory_mib_per_request: 4096,
        per_tag: None,
    };
    let (client, log) = client_replying(&GetAccountLimitsResponse::Limits { limits: limits.clone() })?;
    assert_eq!(client.limits(tenant()?).await?, limits);
    assert_eq!(log.only()?.subject, "sandbox.tenant.limits");

    let denied = GetAccountLimitsResponse::Denied(LeaseError::Backend { message: "down".into() });
    let (client, _) = client_replying(&denied)?;
    assert!(matches!(
        client.limits(tenant()?).await,
        Err(ClientError::Denied(LeaseError::Backend { .. }))
    ));
    Ok(())
}

#[tokio::test]
async fn tenant_usage_returns_the_totals_and_surfaces_denial() -> TestResult {
    let totals = TenantUsage {
        live_leases: 2,
        commands_run: 9,
        ..Default::default()
    };
    let (client, log) = client_replying(&GetTenantUsageResponse::Usage { usage: totals })?;
    assert_eq!(client.tenant_usage(tenant()?).await?, totals);
    assert_eq!(log.only()?.subject, "sandbox.tenant.usage");

    let denied = GetTenantUsageResponse::Denied(LeaseError::Backend { message: "down".into() });
    let (client, _) = client_replying(&denied)?;
    assert!(matches!(
        client.tenant_usage(tenant()?).await,
        Err(ClientError::Denied(LeaseError::Backend { .. }))
    ));
    Ok(())
}
