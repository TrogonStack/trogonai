use russh::keys::PublicKeyBase64;
use trogon_sandbox_ssh_gateway::{HostKeyError, load_or_generate_host_key};

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn generates_then_reloads_the_same_key() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("nested").join("host_key");
    let created = load_or_generate_host_key(&path)?;
    assert!(path.exists());
    let loaded = load_or_generate_host_key(&path)?;
    assert_eq!(
        created.public_key().public_key_base64(),
        loaded.public_key().public_key_base64()
    );
    Ok(())
}

#[test]
fn a_directory_in_place_of_the_key_cannot_be_read() -> TestResult {
    let dir = tempfile::tempdir()?;
    let error = load_or_generate_host_key(dir.path())
        .err()
        .ok_or("expected a failure")?;
    assert!(matches!(error, HostKeyError::Read { .. }));
    Ok(())
}

#[test]
fn garbage_is_not_a_host_key() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("host_key");
    std::fs::write(&path, "not a key")?;
    let error = load_or_generate_host_key(&path).err().ok_or("expected a failure")?;
    assert!(matches!(error, HostKeyError::Decode { .. }));
    Ok(())
}

#[test]
fn a_file_in_place_of_the_parent_directory_cannot_be_created() -> TestResult {
    let dir = tempfile::tempdir()?;
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, "")?;
    let error = load_or_generate_host_key(&blocker.join("host_key"))
        .err()
        .ok_or("expected a failure")?;
    assert!(matches!(error, HostKeyError::CreateDirectory { .. }));
    Ok(())
}

#[test]
fn a_directory_in_place_of_the_key_cannot_hold_it() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("host_key");
    std::fs::create_dir(&path)?;
    let error = load_or_generate_host_key(&path).err().ok_or("expected a failure")?;
    assert!(matches!(error, HostKeyError::Read { .. }));
    Ok(())
}

#[test]
fn an_empty_path_cannot_hold_the_key() -> TestResult {
    let error = load_or_generate_host_key(std::path::Path::new(""))
        .err()
        .ok_or("expected a failure")?;
    assert!(matches!(error, HostKeyError::Write { .. }));
    Ok(())
}
