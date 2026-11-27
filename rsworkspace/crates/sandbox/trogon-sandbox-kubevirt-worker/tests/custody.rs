use std::error::Error;
use std::path::Path;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;

use trogon_sandbox_contract::{LeaseId, TenantId, WorkerId};
use trogon_sandbox_kubevirt_worker::{
    CustodyError, CustodyJournal, CustodyPhase, CustodyResourceKind, CustodyToken, LeaseJournal, LeaseResources,
    ResourceReceipt, WarmCandidate, WarmClaimIntent,
};

type TestResult = Result<(), Box<dyn Error>>;

fn worker() -> Result<WorkerId, Box<dyn Error>> {
    Ok(WorkerId::new("kubevirt-0")?)
}

fn open(path: &Path) -> Result<std::sync::Arc<CustodyJournal>, Box<dyn Error>> {
    Ok(CustodyJournal::open(path, worker()?, "sandboxes")?)
}

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

fn lease() -> LeaseId {
    LeaseId::new("lease-1")
}

fn tenant() -> Result<TenantId, Box<dyn Error>> {
    Ok(TenantId::new("tenant")?)
}

#[test]
fn an_allocation_walks_from_preparing_to_finished() -> TestResult {
    let dir = tempfile::tempdir()?;
    let journal = open(&dir.path().join("state"))?;
    let handle = LeaseJournal::begin(&journal, &lease(), &tenant()?)?;
    assert_eq!(handle.record()?.phase, CustodyPhase::Preparing);
    assert_eq!(journal.all()?.len(), 1);
    assert!(matches!(
        LeaseJournal::begin(&journal, &lease(), &tenant()?),
        Err(CustodyError::NotSettled)
    ));

    assert!(matches!(
        handle.set_phase(CustodyPhase::Ready),
        Err(CustodyError::IncompleteReady)
    ));
    handle.before_create(CustodyResourceKind::VirtualMachine, "vm-a")?;
    assert!(matches!(
        handle.before_create(CustodyResourceKind::DataVolume, "dv-a"),
        Err(CustodyError::CreateUnresolved)
    ));
    assert!(matches!(
        handle.set_phase(CustodyPhase::Ready),
        Err(CustodyError::IncompleteReady)
    ));
    handle.resources(&complete())?;
    assert!(handle.record()?.create.is_none());
    handle.set_phase(CustodyPhase::Ready)?;
    assert!(matches!(handle.finish(), Err(CustodyError::CleanupUnsettled)));

    handle.set_phase(CustodyPhase::Unresolved)?;
    handle.before_create(CustodyResourceKind::DataVolume, "dv-a")?;
    handle.resources(&complete())?;
    assert_eq!(handle.record()?.phase, CustodyPhase::Cleanup);
    handle.resources(&LeaseResources::default())?;
    handle.set_phase(CustodyPhase::Cleaned)?;
    handle.finish()?;
    assert!(journal.get(&lease())?.is_none());
    Ok(())
}

#[test]
fn a_create_for_another_name_stays_unresolved() -> TestResult {
    let dir = tempfile::tempdir()?;
    let journal = open(dir.path())?;
    let handle = LeaseJournal::begin(&journal, &lease(), &tenant()?)?;
    handle.before_create(CustodyResourceKind::VirtualMachine, "vm-b")?;
    handle.resources(&complete())?;
    assert!(handle.record()?.create.is_some());
    assert!(matches!(
        handle.before_create(CustodyResourceKind::VirtualMachine, "bad name"),
        Err(CustodyError::CreateUnresolved)
    ));
    Ok(())
}

#[test]
fn a_replaced_allocation_cannot_be_written_by_the_old_handle() -> TestResult {
    let dir = tempfile::tempdir()?;
    let journal = open(dir.path())?;
    let old = LeaseJournal::begin(&journal, &lease(), &tenant()?)?;
    old.set_phase(CustodyPhase::Cleaned)?;
    old.finish()?;
    let fresh = LeaseJournal::begin(&journal, &lease(), &tenant()?)?;
    assert!(matches!(old.record(), Err(CustodyError::GenerationChanged)));
    let again = LeaseJournal::handle(&journal, &fresh.record()?);
    assert_eq!(again.record()?.lease_id, lease());
    Ok(())
}

#[test]
fn finishing_without_a_record_fails() -> TestResult {
    let dir = tempfile::tempdir()?;
    let journal = open(dir.path())?;
    let handle = LeaseJournal::begin(&journal, &lease(), &tenant()?)?;
    handle.set_phase(CustodyPhase::Cleaned)?;
    handle.finish()?;
    assert!(matches!(handle.finish(), Err(CustodyError::RecordUnavailable)));
    assert!(matches!(
        handle.set_phase(CustodyPhase::Cleanup),
        Err(CustodyError::RecordUnavailable)
    ));
    Ok(())
}

#[test]
fn lease_identities_are_confined_to_file_names() -> TestResult {
    let dir = tempfile::tempdir()?;
    let journal = open(dir.path())?;
    for bad in ["", "a/b", "a.b", &"x".repeat(129)] {
        assert!(matches!(
            journal.get(&LeaseId::new(bad)),
            Err(CustodyError::InvalidLeaseIdentity)
        ));
    }
    Ok(())
}

#[test]
fn a_second_worker_is_refused_while_the_first_holds_the_directory() -> TestResult {
    let dir = tempfile::tempdir()?;
    let first = open(dir.path())?;
    assert!(matches!(
        CustodyJournal::open(dir.path(), worker()?, "sandboxes"),
        Err(CustodyError::AlreadyHeld)
    ));
    drop(first);
    open(dir.path())?;
    Ok(())
}

#[test]
fn a_directory_belongs_to_one_worker_and_namespace() -> TestResult {
    let dir = tempfile::tempdir()?;
    drop(open(dir.path())?);
    assert!(matches!(
        CustodyJournal::open(dir.path(), WorkerId::new("other")?, "sandboxes"),
        Err(CustodyError::ForeignOwner)
    ));
    assert!(matches!(
        CustodyJournal::open(dir.path(), worker()?, "elsewhere"),
        Err(CustodyError::ForeignOwner)
    ));
    assert!(matches!(
        CustodyJournal::open(dir.path(), worker()?, "bad namespace"),
        Err(CustodyError::Namespace(_))
    ));
    let file = dir.path().join("file");
    std::fs::write(&file, b"x")?;
    assert!(matches!(
        CustodyJournal::open(&file, worker()?, "sandboxes"),
        Err(CustodyError::NotDirectory)
    ));
    Ok(())
}

#[test]
fn unrecognized_content_and_foreign_records_are_refused() -> TestResult {
    let dir = tempfile::tempdir()?;
    let journal = open(dir.path())?;
    let handle = LeaseJournal::begin(&journal, &lease(), &tenant()?)?;
    handle.resources(&complete())?;
    let path = dir.path().join("lease-lease-1.json");
    let good = std::fs::read_to_string(&path)?;
    drop((journal, handle));

    std::fs::write(dir.path().join("stray"), b"x")?;
    assert!(matches!(
        CustodyJournal::open(dir.path(), worker()?, "sandboxes"),
        Err(CustodyError::UnrecognizedContent)
    ));
    std::fs::remove_file(dir.path().join("stray"))?;

    std::fs::write(&path, good.replace("kubevirt-0", "someone-else"))?;
    assert!(matches!(
        CustodyJournal::open(dir.path(), worker()?, "sandboxes"),
        Err(CustodyError::ForeignRecord)
    ));
    std::fs::write(&path, good.replace("\"lease-1\"", "\"lease-2\""))?;
    assert!(matches!(
        CustodyJournal::open(dir.path(), worker()?, "sandboxes"),
        Err(CustodyError::LeaseMismatch)
    ));
    std::fs::write(
        &path,
        good.replace("\"sandboxes\",\"name\":\"vm-a\"", "\"elsewhere\",\"name\":\"vm-a\""),
    )?;
    assert!(matches!(
        CustodyJournal::open(dir.path(), worker()?, "sandboxes"),
        Err(CustodyError::InvalidResource)
    ));
    std::fs::write(&path, good.replace("Preparing", "Cleaned"))?;
    assert!(matches!(
        CustodyJournal::open(dir.path(), worker()?, "sandboxes"),
        Err(CustodyError::SettledWithResources)
    ));
    std::fs::write(
        &path,
        good.replace("Preparing", "Ready")
            .replace("\"create\":null", "\"create\":null,\"x\":1"),
    )?;
    assert!(matches!(
        CustodyJournal::open(dir.path(), worker()?, "sandboxes"),
        Err(CustodyError::Json(_))
    ));
    std::fs::write(&path, vec![b' '; 70_000])?;
    assert!(matches!(
        CustodyJournal::open(dir.path(), worker()?, "sandboxes"),
        Err(CustodyError::InvalidFile)
    ));
    std::fs::remove_file(&path)?;
    std::fs::create_dir(&path)?;
    assert!(matches!(
        CustodyJournal::open(dir.path(), worker()?, "sandboxes"),
        Err(CustodyError::InvalidFile)
    ));
    Ok(())
}

#[test]
fn a_ready_record_must_be_complete() -> TestResult {
    let dir = tempfile::tempdir()?;
    let journal = open(dir.path())?;
    let handle = LeaseJournal::begin(&journal, &lease(), &tenant()?)?;
    handle.resources(&complete())?;
    let path = dir.path().join("lease-lease-1.json");
    let good = std::fs::read_to_string(&path)?;
    drop((journal, handle));
    std::fs::write(
        &path,
        good.replace("Preparing", "Ready")
            .replace("\"data_volume\":{", "\"data_volume\":null,\"gone\":{"),
    )?;
    assert!(CustodyJournal::open(dir.path(), worker()?, "sandboxes").is_err());
    Ok(())
}

#[test]
fn warm_claims_are_bound_to_the_lease_scope() -> TestResult {
    let dir = tempfile::tempdir()?;
    let journal = open(dir.path())?;
    let handle = LeaseJournal::begin(&journal, &lease(), &tenant()?)?;
    let record = handle.record()?;
    let candidate = WarmCandidate {
        vm: receipt("vm-a"),
        data_volume: receipt("dv-a"),
        resource_version: "5".into(),
        digest: trogon_sandbox_contract::Digest::new(format!("sha256:{}", "b".repeat(64)))?,
    };
    let claim = |labels, version: &str| LeaseResources {
        pending_claim: Some(WarmClaimIntent {
            candidate: WarmCandidate {
                resource_version: version.into(),
                ..candidate.clone()
            },
            token: CustodyToken::generate(),
            labels,
        }),
        ..LeaseResources::default()
    };
    handle.resources(&claim(journal.labels(&record), "5"))?;
    assert!(matches!(
        handle.resources(&claim(Default::default(), "5")),
        Err(CustodyError::ClaimScope)
    ));
    assert!(matches!(
        handle.resources(&claim(journal.labels(&record), "")),
        Err(CustodyError::InvalidClaim)
    ));
    Ok(())
}

#[test]
fn the_directory_and_files_are_private() -> TestResult {
    let dir = tempfile::tempdir()?;
    let state = dir.path().join("state");
    let journal = open(&state)?;
    LeaseJournal::begin(&journal, &lease(), &tenant()?)?;
    #[cfg(unix)]
    {
        assert_eq!(std::fs::metadata(&state)?.permissions().mode() & 0o777, 0o700);
        assert_eq!(
            std::fs::metadata(state.join("lease-lease-1.json"))?
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    Ok(())
}
