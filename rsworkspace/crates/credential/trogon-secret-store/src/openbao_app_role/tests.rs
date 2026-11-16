use std::path::PathBuf;

use trogon_std::fs::MemFs;

use super::*;

const ROLE_ID: &str = "fake-role-id";
const SECRET_ID: &str = "fake-secret-id-value";
const ROTATED_SECRET_ID: &str = "fake-rotated-secret-id-value";

fn secret_id_path() -> PathBuf {
    PathBuf::from("/run/secrets/openbao-secret-id")
}

fn holding(contents: &str) -> Arc<MemFs> {
    let fs = Arc::new(MemFs::new());
    fs.insert(secret_id_path(), contents);
    fs
}

fn approle(fs: &Arc<MemFs>) -> OpenBaoAppRole {
    OpenBaoAppRole::from_secret_id_file(ROLE_ID, &secret_id_path(), Arc::clone(fs) as _).unwrap()
}

#[test]
fn refuses_an_empty_role_id() {
    let fs = holding(SECRET_ID);

    let error = OpenBaoAppRole::from_secret_id_file("", &secret_id_path(), fs).unwrap_err();

    assert!(matches!(error, OpenBaoAppRoleError::EmptyRoleId));
}

#[test]
fn refuses_a_secret_id_file_that_cannot_be_read() {
    let error = OpenBaoAppRole::from_secret_id_file(ROLE_ID, &secret_id_path(), Arc::new(MemFs::new())).unwrap_err();

    assert!(matches!(error, OpenBaoAppRoleError::SecretIdUnreadable { path, .. } if path == secret_id_path()));
}

#[test]
fn refuses_a_blank_secret_id_file() {
    let error = OpenBaoAppRole::from_secret_id_file(ROLE_ID, &secret_id_path(), holding(" \n")).unwrap_err();

    assert!(matches!(error, OpenBaoAppRoleError::EmptySecretId { path } if path == secret_id_path()));
}

#[test]
fn a_secret_id_file_deleted_after_startup_fails_the_read_naming_only_its_path() {
    let fs = holding(SECRET_ID);
    let approle = approle(&fs);
    fs.remove(&secret_id_path());

    let error = approle.read_secret_id().unwrap_err();

    assert!(matches!(&error, OpenBaoAppRoleError::SecretIdUnreadable { path, .. } if *path == secret_id_path()));
    assert_eq!(
        error.to_string(),
        "failed to read the OpenBao AppRole secret id from /run/secrets/openbao-secret-id: file not found"
    );
}

#[test]
fn a_secret_id_file_blanked_after_startup_fails_the_read_naming_only_its_path() {
    let fs = holding(SECRET_ID);
    let approle = approle(&fs);
    fs.insert(secret_id_path(), "\n");

    let error = approle.read_secret_id().unwrap_err();

    assert!(matches!(&error, OpenBaoAppRoleError::EmptySecretId { path } if *path == secret_id_path()));
    assert_eq!(
        error.to_string(),
        "the OpenBao AppRole secret id file /run/secrets/openbao-secret-id is empty"
    );
}

#[test]
fn reads_the_secret_id_without_its_trailing_newline() {
    let approle = approle(&holding(&format!("{SECRET_ID}\n")));

    assert_eq!(approle.role_id(), ROLE_ID);
    assert_eq!(approle.read_secret_id().unwrap().as_str(), SECRET_ID);
}

#[test]
fn each_read_returns_what_the_file_holds_at_that_moment() {
    let fs = holding(&format!("{SECRET_ID}\n"));
    let approle = approle(&fs);
    assert_eq!(approle.read_secret_id().unwrap().as_str(), SECRET_ID);

    fs.insert(secret_id_path(), format!("{ROTATED_SECRET_ID}\n"));

    assert_eq!(approle.read_secret_id().unwrap().as_str(), ROTATED_SECRET_ID);
}

#[test]
fn debug_never_shows_the_secret_id() {
    let approle = approle(&holding(SECRET_ID));
    let debug = format!("{approle:?}");

    assert!(debug.contains(ROLE_ID));
    assert!(!debug.contains(SECRET_ID));
}
