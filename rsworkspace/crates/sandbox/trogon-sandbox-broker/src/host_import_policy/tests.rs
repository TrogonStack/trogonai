use trogon_sandbox_contract::{HostImport, LeaseError, TenantId};

use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn tenant() -> Result<TenantId, trogon_sandbox_contract::TenantIdError> {
    TenantId::new("acme")
}

fn policy() -> TenantHostImportPolicy {
    TenantHostImportPolicy {
        allow_domains: vec!["example.com".to_string(), "api.acme.io".to_string()],
        max_body_bytes: 1_048_576,
        max_timeout_seconds: 30,
    }
}

async fn make_store() -> Result<InMemoryHostImportPolicy, Box<dyn std::error::Error>> {
    let store = InMemoryHostImportPolicy::new();
    store.insert(&tenant()?, policy()).await;
    Ok(store)
}

#[tokio::test]
async fn allowed_domain_within_caps_ok() -> TestResult {
    let store = make_store().await?;
    let result = store
        .validate(
            &tenant()?,
            &[HostImport::FetchUrl {
                allow_domains: vec!["example.com".to_string()],
                max_body_bytes: 512,
                timeout_seconds: 10,
            }],
        )
        .await;
    assert!(result.is_ok());
    Ok(())
}

#[tokio::test]
async fn domain_not_in_tenant_allowlist_rejected() -> TestResult {
    let store = make_store().await?;
    let result = store
        .validate(
            &tenant()?,
            &[HostImport::FetchUrl {
                allow_domains: vec!["evil.com".to_string()],
                max_body_bytes: 512,
                timeout_seconds: 10,
            }],
        )
        .await;
    assert!(matches!(result, Err(LeaseError::InvalidSpec { .. })));
    Ok(())
}

#[tokio::test]
async fn body_cap_above_tenant_max_rejected() -> TestResult {
    let store = make_store().await?;
    let result = store
        .validate(
            &tenant()?,
            &[HostImport::FetchUrl {
                allow_domains: vec!["example.com".to_string()],
                max_body_bytes: 10_000_000,
                timeout_seconds: 10,
            }],
        )
        .await;
    assert!(matches!(result, Err(LeaseError::InvalidSpec { .. })));
    Ok(())
}

#[tokio::test]
async fn timeout_above_tenant_max_rejected() -> TestResult {
    let store = make_store().await?;
    let result = store
        .validate(
            &tenant()?,
            &[HostImport::FetchUrl {
                allow_domains: vec!["example.com".to_string()],
                max_body_bytes: 512,
                timeout_seconds: 9999,
            }],
        )
        .await;
    assert!(matches!(result, Err(LeaseError::InvalidSpec { .. })));
    Ok(())
}

#[tokio::test]
async fn empty_imports_always_ok_even_without_policy() -> TestResult {
    let store = InMemoryHostImportPolicy::new();
    let result = store.validate(&tenant()?, &[]).await;
    assert!(result.is_ok());
    Ok(())
}

#[tokio::test]
async fn a_tenant_without_a_policy_is_refused() -> TestResult {
    let store = InMemoryHostImportPolicy::default();
    let result = store
        .validate(
            &tenant()?,
            &[HostImport::FetchUrl {
                allow_domains: vec![],
                max_body_bytes: 1,
                timeout_seconds: 1,
            }],
        )
        .await;
    assert!(matches!(result, Err(LeaseError::InvalidSpec { .. })));
    Ok(())
}
