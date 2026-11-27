type TestResult = Result<(), Box<dyn std::error::Error>>;
use super::*;

#[tokio::test]
async fn owner_drop_releases_the_lock_even_with_a_duplicated_descriptor() -> TestResult {
    let tmp = tempfile::tempdir()?;
    let state = StateDir::new(tmp.path().join("state"));
    let owner = StateDirectoryLock::acquire(&state).await?;
    let inherited_descriptor = owner._file.try_clone()?;
    drop(owner);
    let replacement = StateDirectoryLock::acquire(&state).await;
    drop(inherited_descriptor);
    assert!(replacement.is_ok(), "the daemon guard must own the lock lifetime");
    Ok(())
}
