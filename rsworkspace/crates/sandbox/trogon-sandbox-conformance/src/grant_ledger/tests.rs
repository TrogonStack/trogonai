use std::error::Error;
use std::sync::Arc;

use serde_json::json;

use super::*;

fn lease() -> Result<ProviderLease, Box<dyn Error>> {
    Ok(serde_json::from_value(
        json!({ "sandbox_id": "s", "evidence_subject": "e" }),
    )?)
}

#[test]
fn a_new_ledger_is_empty() {
    assert!(GrantLedger::default().snapshot().is_empty());
}

#[test]
fn grants_are_kept_in_arrival_order() -> Result<(), Box<dyn Error>> {
    let ledger = GrantLedger::default();
    let native = lease()?;
    ledger.record(
        ProviderKind::Mock,
        &LeaseId::new("a"),
        &native,
        NativeCleanupAuthority::RequestMatched,
    );
    ledger.record(
        ProviderKind::Mock,
        &LeaseId::new("b"),
        &native,
        NativeCleanupAuthority::ReconciliationRequired,
    );
    let ids: Vec<String> = ledger
        .snapshot()
        .iter()
        .map(|grant| grant.lease_id.to_string())
        .collect();
    assert_eq!(ids, ["a", "b"]);
    Ok(())
}

#[test]
fn a_poisoned_ledger_still_answers() -> Result<(), Box<dyn Error>> {
    let ledger = GrantLedger::default();
    let shared = Arc::clone(&ledger.grants);
    let _ = std::thread::spawn(move || {
        let _guard = shared.lock();
        std::panic::resume_unwind(Box::new("poisoned"));
    })
    .join();
    ledger.record(
        ProviderKind::Mock,
        &LeaseId::new("a"),
        &lease()?,
        NativeCleanupAuthority::RequestMatched,
    );
    assert_eq!(ledger.snapshot().len(), 1);
    Ok(())
}
