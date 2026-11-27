use super::*;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn an_empty_file_goes_when_the_last_holder_releases() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join(".bashrc");
    std::fs::write(&path, b"")?;
    let ledger = PlaceholderLedger::default();
    let hold = ledger.begin();
    hold.record(std::slice::from_ref(&path));
    hold.release();
    assert!(!path.exists());
    Ok(())
}

#[test]
fn removal_waits_for_every_other_holder() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join(".gitconfig");
    std::fs::write(&path, b"")?;
    let ledger = PlaceholderLedger::default();
    let first = ledger.begin();
    let second = ledger.begin();
    first.record(std::slice::from_ref(&path));
    first.release();
    assert!(path.exists());
    second.release();
    assert!(!path.exists());
    Ok(())
}

#[test]
fn a_file_with_content_is_left_alone() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join(".profile");
    std::fs::write(&path, b"export A=1")?;
    let ledger = PlaceholderLedger::default();
    let hold = ledger.begin();
    hold.record(std::slice::from_ref(&path));
    hold.release();
    assert!(path.exists());
    Ok(())
}

#[test]
fn an_empty_directory_goes_after_the_file_inside_it() -> TestResult {
    let dir = tempfile::tempdir()?;
    let nested = dir.path().join("a");
    std::fs::create_dir(&nested)?;
    let file = nested.join(".env");
    std::fs::write(&file, b"")?;
    let ledger = PlaceholderLedger::default();
    let hold = ledger.begin();
    hold.record(&[nested.clone(), file.clone()]);
    hold.release();
    assert!(!file.exists());
    assert!(!nested.exists());
    Ok(())
}

#[test]
fn a_directory_with_content_stays() -> TestResult {
    let dir = tempfile::tempdir()?;
    let nested = dir.path().join("a");
    std::fs::create_dir(&nested)?;
    std::fs::write(nested.join("real"), b"x")?;
    let ledger = PlaceholderLedger::default();
    let hold = ledger.begin();
    hold.record(std::slice::from_ref(&nested));
    hold.release();
    assert!(nested.exists());
    Ok(())
}

#[test]
fn the_exit_sweep_removes_what_a_live_hold_recorded() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join(".zshrc");
    std::fs::write(&path, b"")?;
    let ledger = PlaceholderLedger::default();
    let hold = ledger.begin();
    hold.record(std::slice::from_ref(&path));
    ledger.remove_recorded();
    assert!(!path.exists());
    drop(hold);
    Ok(())
}

#[test]
fn a_dropped_hold_gives_its_count_back() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join(".npmrc");
    std::fs::write(&path, b"")?;
    let ledger = PlaceholderLedger::default();
    let kept = ledger.begin();
    kept.record(std::slice::from_ref(&path));
    drop(ledger.begin());
    assert!(path.exists());
    kept.release();
    assert!(!path.exists());
    Ok(())
}

#[test]
fn a_file_nobody_recorded_stays_after_the_last_release() -> TestResult {
    let dir = tempfile::tempdir()?;
    let recorded = dir.path().join(".bashrc");
    let unrecorded = dir.path().join(".zshrc");
    std::fs::write(&recorded, b"")?;
    std::fs::write(&unrecorded, b"")?;
    let ledger = PlaceholderLedger::default();
    let hold = ledger.begin();
    hold.record(std::slice::from_ref(&recorded));
    hold.release();
    assert!(!recorded.exists());
    assert!(unrecorded.exists());
    Ok(())
}

#[test]
fn a_recorded_file_that_gained_content_meanwhile_stays() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join(".bashrc");
    std::fs::write(&path, b"")?;
    let ledger = PlaceholderLedger::default();
    let hold = ledger.begin();
    hold.record(std::slice::from_ref(&path));
    std::fs::write(&path, b"alias a=b")?;
    hold.release();
    assert_eq!(std::fs::read(&path)?, b"alias a=b");
    Ok(())
}

#[test]
fn a_recorded_path_that_vanished_meanwhile_is_not_an_error() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join(".bashrc");
    std::fs::write(&path, b"")?;
    let ledger = PlaceholderLedger::default();
    let hold = ledger.begin();
    hold.record(std::slice::from_ref(&path));
    std::fs::remove_file(&path)?;
    hold.release();
    assert!(!path.exists());
    Ok(())
}
