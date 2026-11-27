use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn state(worker: &WorkerId, lease: &str) -> Result<CleanupState, Box<dyn std::error::Error>> {
    Ok(CleanupState {
        identity: LeaseAttemptIdentity {
            lease_id: LeaseId::new(lease),
            provider: ProviderKind::Fly,
            target: DispatchTarget {
                epoch: None,
                worker_id: worker.clone(),
                attempt_id: DispatchAttemptId::new("attempt-a")?,
            },
        },
        phase: AttemptPhase::Starting,
        intent: CancelIntent::Release,
        allocation_started: true,
        backend_cleaned: false,
        receipt: None,
    })
}

#[test]
fn a_private_file_cannot_be_created_beneath_a_missing_directory() -> TestResult {
    let dir = tempfile::tempdir()?;
    let error = create_private_file(&dir.path().join("missing").join("ledger"))
        .err()
        .ok_or("expected an error")?;
    assert!(matches!(error, AttemptLedgerError::Io(source) if source.kind() == std::io::ErrorKind::NotFound));
    Ok(())
}

#[test]
fn an_existing_private_file_is_left_alone() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("ledger");
    std::fs::write(&path, b"kept")?;
    create_private_file(&path)?;
    assert_eq!(std::fs::read(&path)?, b"kept");
    Ok(())
}

#[test]
fn a_state_cannot_overwrite_another_lease_under_the_same_attempt_id() -> TestResult {
    let mut database = Connection::open_in_memory()?;
    let worker = initialize(&mut database, ProviderKind::Fly, None)?;
    write_state(&database, &state(&worker, "lease-a")?)?;
    let outcome = write_state(&database, &state(&worker, "lease-b")?);
    assert!(matches!(outcome, Err(AttemptLedgerError::AttemptIdentityReused)));
    Ok(())
}
