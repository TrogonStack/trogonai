mod authority_support;

use std::sync::atomic::Ordering;

use authority_support::{Api, lease_request};
use trogon_sandbox_contract::{LeaseId, SandboxFreezeResponse};
use trogon_sandbox_protocol::{AuthorityHost, FreezeOutcome, Provider, ThawOutcome};

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

#[tokio::test]
async fn a_lost_guard_pause_reply_preserves_ownership_for_recovery_thaw() -> TestResult {
    let mut api = Api::start().await?;
    let provider = api.provider()?;
    let id = LeaseId::new("lost-pause");
    provider.lease(&lease_request()?, &id).await?;
    let held = provider
        .held_leases()
        .await?
        .into_iter()
        .next()
        .ok_or("no held lease")?;
    api.control.drop_pause_replies.store(1, Ordering::SeqCst);
    let pause = provider.freeze(&held).await;
    let paused_after_error = api.control.paused("vm-1");
    let thaw = provider.thaw(&held).await?;
    let resumed = api.control.paused("vm-1");
    let connects = api.control.connects.load(Ordering::SeqCst);
    provider.cancel(&id).await?;
    assert_eq!(api.control.live_count(), 0);
    api.shutdown().await;
    assert!(
        pause.is_err(),
        "the actual API effect happens before its response is lost"
    );
    assert_eq!(paused_after_error, Some(true));
    assert_eq!(
        thaw,
        ThawOutcome::Thawed,
        "uncertain guard pause intent must remain recoverable"
    );
    assert_eq!(resumed, Some(false));
    assert_eq!(connects, 1);
    Ok(())
}

#[tokio::test]
async fn retrying_freeze_recovers_a_guard_pause_with_a_lost_reply() -> TestResult {
    let mut api = Api::start().await?;
    let provider = api.provider()?;
    let id = LeaseId::new("retry-pause");
    provider.lease(&lease_request()?, &id).await?;
    let held = provider
        .held_leases()
        .await?
        .into_iter()
        .next()
        .ok_or("no held lease")?;
    api.control.drop_pause_replies.store(1, Ordering::SeqCst);
    let first = provider.freeze(&held).await;
    let retried = provider.freeze(&held).await?;
    let thawed = provider.thaw(&held).await?;
    let resumed = api.control.paused("vm-1");
    provider.cancel(&id).await?;
    assert_eq!(api.control.live_count(), 0);
    api.shutdown().await;
    assert!(first.is_err());
    assert_eq!(
        retried,
        FreezeOutcome::Frozen,
        "an attempted guard pause is distinct from a preexisting external pause"
    );
    assert_eq!(thawed, ThawOutcome::Thawed);
    assert_eq!(resumed, Some(false));
    Ok(())
}

#[tokio::test]
async fn manual_pause_intent_blocks_guard_resume_even_when_its_reply_is_lost() -> TestResult {
    let mut api = Api::start().await?;
    let provider = api.provider()?;
    let id = LeaseId::new("manual-pause");
    provider.lease(&lease_request()?, &id).await?;
    let held = provider
        .held_leases()
        .await?
        .into_iter()
        .next()
        .ok_or("no held lease")?;
    assert_eq!(provider.freeze(&held).await?, FreezeOutcome::Frozen);
    api.control.drop_pause_replies.store(1, Ordering::SeqCst);
    let manual = provider.set_paused_for_lease(&id, true).await;
    let thaw = provider.thaw(&held).await?;
    let paused = api.control.paused("vm-1");
    let connects = api.control.connects.load(Ordering::SeqCst);
    provider.cancel(&id).await?;
    assert_eq!(api.control.live_count(), 0);
    api.shutdown().await;
    assert!(matches!(manual, SandboxFreezeResponse::Denied(_)));
    assert_eq!(
        thaw,
        ThawOutcome::NoLongerOwned,
        "manual intent invalidates the guard's pause before the RPC reply"
    );
    assert_eq!(paused, Some(true));
    assert_eq!(connects, 0);
    Ok(())
}
