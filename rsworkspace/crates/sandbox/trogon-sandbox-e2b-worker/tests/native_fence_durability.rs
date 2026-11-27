#![cfg(target_os = "linux")]

mod authority_support;
mod ensure_support;
mod native_access_support;
mod native_command_support;
mod native_fence_durability_support;
mod native_process_support;
mod worker_process_support;

use ensure_support::{Boxed, Context};
use native_fence_durability_support::{FenceSwap, Fixture};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn replaced_canonical_fence_refuses_fresh_strong_effects_but_preserves_old_reconciliation() -> Result<(), Boxed> {
    let mut fixture = Fixture::start(false).await?;
    let mut assigned = None;
    let mut swap = None;
    let observed = async {
        let heartbeat = fixture.readiness().await?;
        let assigned = assigned.insert(fixture.admit(&heartbeat).await?);
        fixture.ordinary(assigned).await?;
        let sentinel = fixture.sentinel(assigned).await?;
        let (execution, original) = fixture.original(assigned).await?;
        assert!(
            original.pid != sentinel.pid
                && !fixture.guest.group_members(original)?.is_empty()
                && !fixture.guest.group_members(sentinel)?.is_empty(),
            "intact strong native launch and sentinel must be independent"
        );
        let authority = fixture.authority(assigned).await?;
        let installed = swap.insert(FenceSwap::replace(&fixture.fence_path())?);
        assert!(
            installed.original_bytes()? == authority.fence && installed.replacement_empty()?,
            "path fault must retain the original journal and install only a distinct empty canonical file"
        );
        let fresh = fixture.fresh(assigned).await?;
        let old_canceled = fixture.cancel_original(assigned, &execution).await?;
        let old_absent = fixture.guest.group_members(original)?.is_empty();
        let sentinel_preserved = !fixture.guest.group_members(sentinel)?.is_empty();
        let authority_preserved = fixture.authority_preserved(assigned, &authority).await?;
        let fault_still_present = installed.replacement_empty()? && installed.original_bytes()? == authority.fence;
        Ok::<_, Boxed>((
            fresh,
            old_canceled,
            old_absent,
            sentinel_preserved,
            authority_preserved,
            fault_still_present,
        ))
    }
    .await;
    if let Some(swap) = &mut swap {
        swap.restore()?;
    }
    let cleanup = fixture.finish(assigned.as_ref()).await?;
    let (fresh, old_canceled, old_absent, sentinel_preserved, authority_preserved, fault_still_present) =
        observed.context("owned worker stopped; original fence restored; exact fixture native cleanup settled")?;
    assert!(
        cleanup.product && cleanup.native_created,
        "the positive held lease must end in exact targeted product native cleanup"
    );
    assert!(
        old_canceled && old_absent && sentinel_preserved && authority_preserved && fault_still_present,
        "journal path failure must preserve the original exact cancellation/replay authority before restoration"
    );
    assert!(
        fresh.refused && fresh.bootstraps_after == fresh.bootstraps_before && !fresh.effect,
        "a replaced canonical fence journal must refuse a fresh strong launch before native bootstrap or user effect"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn configured_unopenable_fence_cannot_earn_fresh_strong_native_effects() -> Result<(), Boxed> {
    let mut intact = Fixture::start(false).await?;
    let mut positive_assignment = None;
    let positive = async {
        let heartbeat = intact.readiness().await?;
        let assigned = positive_assignment.insert(intact.admit(&heartbeat).await?);
        intact.ordinary(assigned).await?;
        let (execution, _) = intact.original(assigned).await?;
        assert!(
            intact.cancel_original(assigned, &execution).await?,
            "intact positive strong cancellation must settle and replay"
        );
        Ok::<_, Boxed>(())
    }
    .await;
    let positive_cleanup = intact.finish(positive_assignment.as_ref()).await?;
    positive.context("intact positive fixture native cleanup settled")?;
    assert!(
        positive_cleanup.product,
        "intact strong positive requires exact product cleanup"
    );

    let mut fixture = Fixture::start(true).await?;
    let mut assigned = None;
    let observed = async {
        assert!(
            fixture.fence_path().is_dir(),
            "configured fence must be an unopenable file path"
        );
        let heartbeat = fixture.readiness().await?;
        let assigned = assigned.insert(fixture.admit(&heartbeat).await?);
        fixture.ordinary(assigned).await?;
        fixture.fresh(assigned).await
    }
    .await;
    let cleanup = fixture.finish(assigned.as_ref()).await?;
    let fresh = observed.context("fallback worker stopped; exact owned fixture cleanup settled")?;
    assert!(
        cleanup.native_created && cleanup.product,
        "a granted native lease requires product cleanup"
    );
    assert!(
        fresh.refused && fresh.bootstraps_after == fresh.bootstraps_before && !fresh.effect,
        "configured fence open failure must not silently earn fresh strong effects through memory-only verification"
    );
    Ok(())
}
