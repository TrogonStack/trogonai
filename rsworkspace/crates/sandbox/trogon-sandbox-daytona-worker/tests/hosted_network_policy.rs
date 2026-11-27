//! Network policy on Daytona: what the worker sends at create time, and what the provider records for a lease that
//! asked for no egress.
//!
//! The lane is skipped unless `SANDBOX_DAYTONA_HOSTED_TEST=1`; see `hosted_support` for the other variables.

mod hosted_support;
#[path = "hosted_support/network_policy_observation/mod.rs"]
mod network_policy_observation;
#[path = "hosted_support/suite_failure_diagnosis/mod.rs"]
mod suite_failure_diagnosis;

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use hosted_support::{Harness, LANE_BOUND, TestResult, finish};
use network_policy_observation::{Observation, Owner, Report, parse_owned};
use trogon_sandbox_conformance::{GrantLedger, SuiteFailureError, checks_for_target};
use trogon_sandbox_contract::NetworkPolicy;
use trogon_sandbox_daytona_worker::{NewSandbox, translate_network_policy};

const EGRESS_CHECK: &str = "denied_connect_fails";
const POLL: Duration = Duration::from_millis(500);

#[test]
fn deny_all_translates_to_a_block_and_allow_all_to_nothing() -> TestResult {
    let blocked = translate_network_policy(Some(&NetworkPolicy::DenyAll))?;
    assert_eq!(blocked.block_all, Some(true));
    assert_eq!(blocked.allow_list, None);

    let open = translate_network_policy(Some(&NetworkPolicy::AllowAll))?;
    assert_ne!(open.block_all, Some(true));
    assert_eq!(open.allow_list, None);
    Ok(())
}

#[test]
fn a_create_request_carries_only_the_settings_it_was_given() -> TestResult {
    let mut request = NewSandbox::new("base");
    let bare = serde_json::to_value(&request)?;
    assert!(bare.get("networkBlockAll").is_none());
    assert!(bare.get("networkAllowList").is_none());

    request.network_block_all = Some(true);
    let blocked = serde_json::to_value(&request)?;
    assert_eq!(blocked.get("networkBlockAll"), Some(&serde_json::Value::Bool(true)));
    assert!(blocked.get("networkAllowList").is_none());
    Ok(())
}

async fn observe_leases(harness: &Harness, ledger: &GrantLedger, done: &AtomicBool) -> Report {
    let mut observed: Vec<Observation> = Vec::new();
    let mut total = 0;
    loop {
        let grants = ledger.snapshot();
        total = total.max(grants.len());
        for grant in grants.iter().skip(observed.len()).take(2 - observed.len().min(2)) {
            let owner = Owner {
                sandbox_id: grant.native.sandbox_id.as_str(),
                lease_id: grant.lease_id.as_str(),
                organization_id: &harness.native.organization_id,
            };
            let seen = match harness.native.record(owner.sandbox_id).await {
                Ok((status, body)) => parse_owned(status, &body, &owner),
                Err(_) => Observation::RequestFailed,
            };
            observed.push(seen);
        }
        if observed.len() >= 2 || done.load(Ordering::SeqCst) {
            break;
        }
        tokio::time::sleep(POLL).await;
    }
    let missing = Observation::ReconciliationRequired;
    Report {
        allow_all_control: observed.first().copied().unwrap_or(missing),
        deny_all_restricted: observed.get(1).copied().unwrap_or(missing),
        unexpected_grant_count: total.saturating_sub(2),
    }
}

#[tokio::test]
#[ignore = "runs against a real Daytona account; set SANDBOX_DAYTONA_HOSTED_TEST=1 and use --ignored"]
async fn hosted_original_egress_check_with_owned_raw_policy_presence() -> TestResult {
    let harness = Harness::configured().await?;
    let ledger = GrantLedger::default();
    let target = harness.target(ledger.clone())?;
    let check = checks_for_target(&target)
        .into_iter()
        .find(|(name, _)| *name == EGRESS_CHECK)
        .map(|(_, check)| check)
        .ok_or("the worker's profile does not select the egress check")?;
    let done = AtomicBool::new(false);
    let run = async {
        let result = check(&target).await;
        done.store(true, Ordering::SeqCst);
        result
    };
    let (outcome, report) = tokio::time::timeout(LANE_BOUND, async {
        tokio::join!(run, observe_leases(&harness, &ledger, &done))
    })
    .await
    .map_err(|_| "the egress lane did not finish within its bound")?;
    let failures = match outcome {
        Ok(()) => Vec::new(),
        Err(failure) => vec![SuiteFailureError {
            name: EGRESS_CHECK,
            failure,
        }],
    };
    println!(
        "{}",
        serde_json::json!({
            "receipt": "network_policy",
            "control": format!("{:?}", report.allow_all_control),
            "restricted": format!("{:?}", report.deny_all_restricted),
            "unexpected_grants": report.unexpected_grant_count,
            "complete": report.complete(),
        })
    );
    let settled = finish(&harness, &ledger, "network_policy", Some(failures)).await;
    if !report.complete() {
        return Err("the provider records did not show the expected network policy for both leases".into());
    }
    settled
}
