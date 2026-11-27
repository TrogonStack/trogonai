use super::*;

#[test]
fn a_dropped_guard_is_no_longer_held() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let guard = ExitGuard::hold(dir.path());
    assert!(HELD.lock()?.iter().any(|p| p == dir.path()));
    drop(guard);
    assert!(!HELD.lock()?.iter().any(|p| p == dir.path()));
    Ok(())
}

#[test]
fn what_is_held_is_removed_by_the_exit_hook() -> Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().to_path_buf();
    let _guard = ExitGuard::hold(&path);
    remove_all(&HELD.lock()?);
    assert!(!path.exists());
    Ok(())
}
