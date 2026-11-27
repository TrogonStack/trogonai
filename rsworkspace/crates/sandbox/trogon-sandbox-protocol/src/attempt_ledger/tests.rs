use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn contention_means_another_worker_owns_the_ledger() {
    assert!(matches!(
        lock_error(TryLockError::WouldBlock),
        AttemptLedgerError::Locked
    ));
}

#[test]
fn any_other_lock_failure_is_an_io_failure() -> TestResult {
    let error = lock_error(TryLockError::Error(std::io::Error::other("no locks")));
    assert!(matches!(&error, AttemptLedgerError::Io(source) if source.kind() == std::io::ErrorKind::Other));
    Ok(())
}
