use std::error::Error;

use serde_json::Value;
use trogon_sandbox_contract::{LeaseId, TenantId, WorkerId};
use trogon_sandbox_kubevirt_worker::{
    CustodyError, CustodyJournal, CustodyPhase, LeaseJournal, LeaseResources, ResourceReceipt,
};

type TestResult = Result<(), Box<dyn Error>>;

const RECORD_LIMIT: usize = 4096;

fn receipt(name: &str) -> ResourceReceipt {
    ResourceReceipt {
        namespace: "sandboxes".into(),
        name: name.into(),
        uid: format!("{name}-uid"),
    }
}

fn complete() -> LeaseResources {
    LeaseResources {
        vm: Some(receipt("vm-a")),
        data_volume: Some(receipt("dv-a")),
        pending_claim: None,
    }
}

#[test]
fn a_ready_record_on_disk_must_name_both_resources() -> TestResult {
    let dir = tempfile::tempdir()?;
    let journal = CustodyJournal::open(dir.path(), WorkerId::new("kubevirt-0")?, "sandboxes")?;
    let handle = LeaseJournal::begin(&journal, &LeaseId::new("lease-1"), &TenantId::new("tenant")?)?;
    handle.resources(&complete())?;
    let path = dir.path().join("lease-lease-1.json");
    let mut value: Value = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
    value["phase"] = Value::from("Ready");
    value["resources"]["data_volume"] = Value::Null;
    drop((journal, handle));
    std::fs::write(&path, serde_json::to_vec(&value)?)?;
    assert!(matches!(
        CustodyJournal::open(dir.path(), WorkerId::new("kubevirt-0")?, "sandboxes"),
        Err(CustodyError::IncompleteReady)
    ));
    Ok(())
}

#[test]
fn a_full_journal_refuses_another_allocation() -> TestResult {
    let dir = tempfile::tempdir()?;
    let worker = WorkerId::new("kubevirt-0")?;
    let tenant = TenantId::new("tenant")?;
    let journal = CustodyJournal::open(dir.path(), worker.clone(), "sandboxes")?;
    LeaseJournal::begin(&journal, &LeaseId::new("seed"), &tenant)?;
    let template = std::fs::read_to_string(dir.path().join("lease-seed.json"))?;
    drop(journal);
    for index in 0..RECORD_LIMIT - 1 {
        let lease = format!("l{index}");
        std::fs::write(
            dir.path().join(format!("lease-{lease}.json")),
            template.replace("\"seed\"", &format!("\"{lease}\"")),
        )?;
    }
    let journal = CustodyJournal::open(dir.path(), worker, "sandboxes")?;
    assert_eq!(journal.all()?.len(), RECORD_LIMIT);
    assert!(matches!(
        LeaseJournal::begin(&journal, &LeaseId::new("extra"), &tenant),
        Err(CustodyError::CapacityExhausted)
    ));
    assert_eq!(
        journal.get(&LeaseId::new("seed"))?.map(|record| record.phase),
        Some(CustodyPhase::Preparing)
    );
    Ok(())
}
