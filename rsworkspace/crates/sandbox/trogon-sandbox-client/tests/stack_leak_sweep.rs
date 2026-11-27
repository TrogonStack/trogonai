//! Fails when the tenant the stack lanes use still holds a live lease.
//!
//! Run it after the other lanes, against the same broker, with `--ignored`. `SANDBOX_SMOKE_TENANT` names the tenant
//! (default `smoke-tenant`). See `stack_support` for how the lane finds NATS.

mod common;
mod stack_support;

use common::TestResult;
use trogon_sandbox_contract::{LeaseStatus, ListLeasesRequest};

/// The largest page the broker serves.
const PAGE_LIMIT: u32 = 200;

/// The statuses a lease holds while it still has, or may still get, a running sandbox.
fn is_live(status: LeaseStatus) -> bool {
    matches!(
        status,
        LeaseStatus::Pending
            | LeaseStatus::Running
            | LeaseStatus::Paused
            | LeaseStatus::Suspending
            | LeaseStatus::Waking
            | LeaseStatus::Cancelling
    )
}

#[tokio::test]
#[ignore = "requires a running stack; run it after the other stack lanes"]
async fn no_live_leases_remain_for_the_smoke_tenant() -> TestResult {
    let tenant = stack_support::smoke_tenant()?;
    let client = stack_support::connect().await?;

    let mut leaked = Vec::new();
    let mut cursor = None;
    loop {
        let page = client
            .list_leases(ListLeasesRequest {
                tenant: tenant.clone(),
                status_filter: None,
                cursor: cursor.take(),
                limit: Some(PAGE_LIMIT),
                tags: Default::default(),
            })
            .await?;
        leaked.extend(
            page.leases
                .into_iter()
                .filter(|lease| is_live(lease.status))
                .map(|lease| lease.lease_id),
        );
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => break,
        }
    }

    assert!(
        leaked.is_empty(),
        "{} lease(s) still live for tenant {}: {leaked:?}",
        leaked.len(),
        tenant.as_str()
    );
    Ok(())
}
