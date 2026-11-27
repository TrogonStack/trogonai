mod authority_support;

use std::sync::atomic::Ordering;

use authority_support::{Api, lease_request};

use trogon_sandbox_contract::LeaseId;
use trogon_sandbox_protocol::{AuthorityHost, Provider, ThawOutcome};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
async fn a_rejected_guard_pause_cannot_claim_a_later_external_pause() -> TestResult {
    let mut api = Api::start().await?;
    let provider = api.provider()?;
    let id = LeaseId::new("rejected-pause");
    provider.lease(&lease_request()?, &id).await?;
    let held = provider
        .held_leases()
        .await?
        .into_iter()
        .next()
        .ok_or("no held lease")?;
    api.control.reject_pause_replies.store(1, Ordering::SeqCst);
    let rejected = provider.freeze(&held).await;
    let state_after_rejection = api.control.paused("vm-1");
    api.control.external_pause("vm-1");
    let thaw = provider.thaw(&held).await?;
    let paused = api.control.paused("vm-1");
    let connects = api.control.connects.load(Ordering::SeqCst);
    provider.cancel(&id).await?;
    assert_eq!(api.control.live_count(), 0);
    api.shutdown().await;
    assert!(rejected.is_err());
    assert_eq!(state_after_rejection, Some(false));
    assert_eq!(
        thaw,
        ThawOutcome::NoLongerOwned,
        "a definite native rejection cannot establish pause ownership"
    );
    assert_eq!(paused, Some(true));
    assert_eq!(connects, 0);
    Ok(())
}
