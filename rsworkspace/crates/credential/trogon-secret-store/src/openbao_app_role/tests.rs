use std::path::PathBuf;

use trogon_std::fs::MemFs;

use super::*;

const ROLE_ID: &str = "fake-role-id";
const SECRET_ID: &str = "fake-secret-id-value";

fn secret_id_path() -> PathBuf {
    PathBuf::from("/run/secrets/openbao-secret-id")
}

#[test]
fn refuses_an_empty_role_id() {
    let fs = MemFs::new();
    fs.insert(secret_id_path(), SECRET_ID);

    let error = OpenBaoAppRole::from_secret_id_file("", &secret_id_path(), &fs).unwrap_err();

    assert!(matches!(error, OpenBaoAppRoleError::EmptyRoleId));
}

#[test]
fn refuses_a_secret_id_file_that_cannot_be_read() {
    let error = OpenBaoAppRole::from_secret_id_file(ROLE_ID, &secret_id_path(), &MemFs::new()).unwrap_err();

    assert!(matches!(error, OpenBaoAppRoleError::SecretIdUnreadable { path, .. } if path == secret_id_path()));
}

#[test]
fn refuses_a_blank_secret_id_file() {
    let fs = MemFs::new();
    fs.insert(secret_id_path(), " \n");

    let error = OpenBaoAppRole::from_secret_id_file(ROLE_ID, &secret_id_path(), &fs).unwrap_err();

    assert!(matches!(error, OpenBaoAppRoleError::EmptySecretId { path } if path == secret_id_path()));
}

#[test]
fn reads_the_secret_id_without_its_trailing_newline() {
    let fs = MemFs::new();
    fs.insert(secret_id_path(), format!("{SECRET_ID}\n"));

    let approle = OpenBaoAppRole::from_secret_id_file(ROLE_ID, &secret_id_path(), &fs).unwrap();

    assert_eq!(approle.role_id(), ROLE_ID);
    assert_eq!(approle.secret_id().as_str(), SECRET_ID);
}

#[test]
fn debug_never_shows_the_secret_id() {
    let fs = MemFs::new();
    fs.insert(secret_id_path(), SECRET_ID);

    let approle = OpenBaoAppRole::from_secret_id_file(ROLE_ID, &secret_id_path(), &fs).unwrap();
    let debug = format!("{approle:?}");

    assert!(debug.contains(ROLE_ID));
    assert!(!debug.contains(SECRET_ID));
}
