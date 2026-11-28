mod provider_support;
mod support;

use provider_support::*;
use serde_json::json;
use trogon_sandbox_protocol::Provider;
use trogon_sandbox_vercel_worker::VercelApiError;

const SANDBOX: &str = "/v2/sandboxes/sbx-a";

#[tokio::test]
async fn an_allow_list_reaches_the_create_body_as_sorted_hosts_and_subnets() -> TestResult {
    let fake = Fake::start().await?;
    fake.created();
    let request = request_with(|wire| {
        wire["spec"]["exposed_ports"] = json!([3000]);
        wire["spec"]["network_policy"] = json!({
            "user_defined": {
                "rules": [{"domain": "b.example.com"}, {"domain": "a.example.com"}],
                "allow_ranges": ["192.168.0.0/16"],
                "deny_ranges": ["192.168.9.0/24"],
                "opaque_tunnels": "allow"
            }
        });
    })?;
    within(fake.provider()?.lease(&request, &lease_id("l1"))).await??;
    let calls = fake.calls("POST", "/v2/sandboxes");
    let body = &calls.first().ok_or("no create call")?.body;
    assert_eq!(
        body["networkPolicy"],
        json!({
            "allow": ["a.example.com", "b.example.com"],
            "subnets": {"allow": ["192.168.0.0/16"], "deny": ["192.168.9.0/24"]}
        })
    );
    assert_eq!(body["ports"], json!([3000]));
    Ok(())
}

#[tokio::test]
async fn the_native_sandbox_is_present_while_leased_and_absent_once_released() -> TestResult {
    let fake = Fake::start().await?;
    fake.created();
    fake.script(
        "GET",
        SANDBOX,
        vec![
            (200, record("sess_1", "https://one.example.test")),
            (404, json!({"error": {"message": "gone"}})),
        ],
    );
    fake.script(
        "DELETE",
        SANDBOX,
        vec![(200, json!({})), (404, json!({"error": {"message": "gone"}}))],
    );
    let provider = fake.provider()?;
    let request = request_with(|wire| wire["spec"]["network_policy"] = json!("deny_all"))?;
    within(provider.lease(&request, &lease_id("l1"))).await??;

    let api = fake.server.api()?;
    let control = api.get_sandbox("sbx-a", false).await?;
    assert_eq!(
        (control.sandbox.name.as_str(), control.session.id.as_str()),
        ("sbx-a", "sess_1")
    );
    assert_eq!(control.session.status, "running");

    within(provider.cancel(&lease_id("l1"))).await??;
    assert_eq!(provider.active_lease_count().await, 0);
    assert_eq!(fake.count("DELETE", SANDBOX), 1);
    assert!(matches!(
        api.get_sandbox("sbx-a", false).await,
        Err(VercelApiError::NotFound(_))
    ));
    assert!(!api.delete_sandbox("sbx-a").await?);
    Ok(())
}

#[tokio::test]
async fn a_sandbox_that_cannot_be_deleted_stays_leased_until_the_delete_succeeds() -> TestResult {
    let fake = Fake::start().await?;
    fake.created();
    fake.script(
        "DELETE",
        SANDBOX,
        vec![(503, json!({"error": {"message": "busy"}})), (200, json!({}))],
    );
    let provider = fake.provider()?;
    within(provider.lease(&request()?, &lease_id("l1"))).await??;
    assert!(within(provider.cancel(&lease_id("l1"))).await?.is_err());
    assert_eq!(provider.active_lease_count().await, 1);
    within(provider.cancel(&lease_id("l1"))).await??;
    assert_eq!(provider.active_lease_count().await, 0);
    Ok(())
}
