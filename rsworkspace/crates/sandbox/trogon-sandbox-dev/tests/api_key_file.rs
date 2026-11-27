use std::error::Error;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use trogon_sandbox_auth::BearerToken;
use trogon_sandbox_dev::{DevError, generate_api_key, write_api_key_file};

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn generated_keys_are_distinct_dev_keys() -> TestResult {
    let first = generate_api_key()?;
    let second = generate_api_key()?;
    assert!(first.expose_secret().starts_with("sbx-dev-"));
    assert_eq!(first.expose_secret().len(), "sbx-dev-".len() + 32);
    assert_ne!(first.expose_secret(), second.expose_secret());
    Ok(())
}

#[cfg(unix)]
#[test]
fn the_key_file_is_owner_only() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("api-key");
    write_api_key_file(&path, &BearerToken::new("token-value")?)?;
    assert_eq!(std::fs::read_to_string(&path)?, "token-value");
    assert_eq!(std::fs::metadata(&path)?.permissions().mode() & 0o777, 0o600);
    Ok(())
}

#[test]
fn an_existing_file_is_never_overwritten() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("existing");
    std::fs::write(&path, b"keep me")?;
    let error = write_api_key_file(&path, &BearerToken::new("token-value")?);
    assert!(matches!(error, Err(DevError::ApiKeyFile { .. })));
    assert_eq!(std::fs::read(&path)?, b"keep me");
    Ok(())
}

#[cfg(unix)]
#[test]
fn a_symlink_is_refused_without_touching_its_target() -> TestResult {
    let dir = tempfile::tempdir()?;
    let target = dir.path().join("target");
    let link = dir.path().join("link");
    std::fs::write(&target, b"keep me")?;
    std::os::unix::fs::symlink(&target, &link)?;
    assert!(write_api_key_file(&link, &BearerToken::new("token-value")?).is_err());
    assert_eq!(std::fs::read(&target)?, b"keep me");
    Ok(())
}
