mod support;

use support::{API_KEY, TENANT, TestResult, api_keys};
use trogon_sandbox_ssh_gateway::validate_bearer;

#[tokio::test]
async fn a_known_key_names_its_tenant_and_service_account() -> TestResult {
    let store = api_keys().await?;
    let found = validate_bearer(store.as_ref(), API_KEY).await?.ok_or("key not found")?;
    assert_eq!(found.0.as_str(), TENANT);
    assert_eq!(found.1.as_str(), "sa-test");
    Ok(())
}

#[tokio::test]
async fn an_unknown_key_is_not_authenticated() -> TestResult {
    let store = api_keys().await?;
    assert!(validate_bearer(store.as_ref(), "other").await?.is_none());
    Ok(())
}

#[tokio::test]
async fn an_empty_key_is_not_authenticated() -> TestResult {
    let store = api_keys().await?;
    assert!(validate_bearer(store.as_ref(), "").await?.is_none());
    Ok(())
}
