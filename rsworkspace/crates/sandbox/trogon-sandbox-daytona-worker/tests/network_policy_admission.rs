mod support;

use serde_json::json;
use support::*;
use trogon_sandbox_contract::LeaseError;
use trogon_sandbox_daytona_worker::DaytonaProviderError;
use trogon_sandbox_protocol::{Provider, ProviderError};

fn is_refused(error: &ProviderError) -> bool {
    match error {
        ProviderError::Backend(inner) => matches!(
            inner.downcast_ref::<DaytonaProviderError>(),
            Some(DaytonaProviderError::Lease(LeaseError::InvalidNetworkPolicy { reason }))
                if reason == "restricted Daytona egress requires verified provider enforcement, which is unavailable"
        ),
        _ => false,
    }
}

#[tokio::test]
async fn unverified_restricted_egress_is_refused_before_provider_effects() -> TestResult {
    let fake = Fake::with_sandbox().await?;
    let provider = fake.provider()?;
    let ordinary = request_with(|wire| wire["spec"]["network_policy"] = json!("allow_all"))?;
    provider.lease(&ordinary, &lease_id("network-ordinary")).await?;
    assert_eq!(fake.count("POST", "/sandbox"), 1, "a healthy provider creates");
    provider.cancel(&lease_id("network-ordinary")).await?;

    let user_defined = json!({"user_defined": {
        "rules": [],
        "allow_ranges": ["203.0.113.0/24"],
        "deny_ranges": [],
        "denied_domains": [],
    }});
    for policy in [json!("deny_all"), user_defined] {
        let before = fake.total();
        let restricted = request_with(|wire| wire["spec"]["network_policy"] = policy.clone())?;
        let refused = provider
            .lease(&restricted, &lease_id("network-restricted"))
            .await
            .err()
            .ok_or("a restricted policy was admitted")?;
        assert!(is_refused(&refused), "{refused}");
        assert_eq!(fake.total(), before, "the refusal must come before any Daytona call");
        assert_eq!(provider.active_lease_count().await, 0);
    }
    Ok(())
}
