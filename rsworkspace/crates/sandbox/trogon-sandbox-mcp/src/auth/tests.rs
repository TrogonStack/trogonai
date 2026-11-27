use std::error::Error;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use trogon_std::env::InMemoryEnv;

use super::{default_api_key_path, read_api_key_file, resolve_api_key};
use crate::auth_error::AuthError;

fn key_file(home: &Path, mode: u32, contents: &str) -> Result<(), Box<dyn Error>> {
    let dir = home.join(".config").join("trogon-sandbox-mcp");
    fs::create_dir_all(&dir)?;
    let file = dir.join("api-key");
    fs::write(&file, contents)?;
    fs::set_permissions(&file, fs::Permissions::from_mode(mode))?;
    Ok(())
}

fn env_with_home(home: &Path) -> InMemoryEnv {
    let env = InMemoryEnv::new();
    env.set("HOME", home.to_string_lossy().to_string());
    env
}

#[test]
fn the_command_line_key_wins() {
    let env = InMemoryEnv::new();
    assert_eq!(resolve_api_key(Some("cli".to_string()), &env), Some("cli".to_string()));
}

#[test]
fn an_empty_command_line_key_falls_back_to_the_file() -> Result<(), Box<dyn Error>> {
    let home = tempfile::tempdir()?;
    key_file(home.path(), 0o600, "  from-file \n")?;
    let env = env_with_home(home.path());
    assert_eq!(
        resolve_api_key(Some(String::new()), &env),
        Some("from-file".to_string())
    );
    Ok(())
}

#[test]
fn no_key_anywhere_is_none() -> Result<(), Box<dyn Error>> {
    let home = tempfile::tempdir()?;
    let env = env_with_home(home.path());
    assert_eq!(resolve_api_key(None, &env), None);
    Ok(())
}

#[test]
fn the_path_falls_back_to_userprofile_then_the_working_directory() {
    let env = InMemoryEnv::new();
    env.set("USERPROFILE", "/profile");
    assert_eq!(
        default_api_key_path(&env),
        Path::new("/profile/.config/trogon-sandbox-mcp/api-key")
    );
    assert_eq!(
        default_api_key_path(&InMemoryEnv::new()),
        Path::new("./.config/trogon-sandbox-mcp/api-key")
    );
}

#[test]
fn a_missing_file_is_not_a_file() -> Result<(), Box<dyn Error>> {
    let home = tempfile::tempdir()?;
    let error = read_api_key_file(&home.path().join("nope"))
        .err()
        .ok_or("expected an error")?;
    assert!(matches!(error, AuthError::NotAFile { .. }));
    Ok(())
}

#[test]
fn a_file_others_can_read_is_refused() -> Result<(), Box<dyn Error>> {
    let home = tempfile::tempdir()?;
    key_file(home.path(), 0o644, "secret")?;
    let path = default_api_key_path(&env_with_home(home.path()));
    let error = read_api_key_file(&path).err().ok_or("expected an error")?;
    assert!(matches!(error, AuthError::InsecureFileMode { mode: 0o644, .. }));
    Ok(())
}

#[test]
fn an_empty_file_is_refused() -> Result<(), Box<dyn Error>> {
    let home = tempfile::tempdir()?;
    key_file(home.path(), 0o600, " \n")?;
    let path = default_api_key_path(&env_with_home(home.path()));
    let error = read_api_key_file(&path).err().ok_or("expected an error")?;
    assert!(matches!(error, AuthError::Empty { .. }));
    Ok(())
}

#[test]
fn a_file_that_cannot_be_read_names_the_path() -> Result<(), Box<dyn Error>> {
    let home = tempfile::tempdir()?;
    key_file(home.path(), 0o600, "secret")?;
    let path = default_api_key_path(&env_with_home(home.path()));
    fs::write(&path, [0xff, 0xfe])?;
    let error = read_api_key_file(&path).err().ok_or("expected an error")?;
    assert!(matches!(error, AuthError::ReadFile { .. }));
    Ok(())
}
