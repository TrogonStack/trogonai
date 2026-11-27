use std::error::Error;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use serde::Serialize;
use serde::Serializer;
use serde::ser::Error as _;

use super::{policy_json, utf8_path, write_policy_file};
use crate::DockerError;

struct Unserializable;

impl Serialize for Unserializable {
    fn serialize<S: Serializer>(&self, _serializer: S) -> Result<S::Ok, S::Error> {
        Err(S::Error::custom("refused"))
    }
}

#[test]
fn unserializable_policy_is_reported() {
    assert!(matches!(
        policy_json(&Unserializable),
        Err(DockerError::PolicySerialize(_))
    ));
}

#[test]
fn utf8_paths_pass_through() -> Result<(), DockerError> {
    assert_eq!(utf8_path("x", Path::new("/a/b"))?, "/a/b");
    Ok(())
}

#[cfg(unix)]
#[test]
fn non_utf8_paths_are_rejected() {
    let path = Path::new(std::ffi::OsStr::from_bytes(b"/a/\xff"));
    assert!(matches!(
        utf8_path("ca file", path),
        Err(DockerError::NonUtf8Path { what: "ca file", .. })
    ));
}

#[tokio::test]
async fn policy_file_lands_in_the_given_directory() -> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let path = write_policy_file(dir.path(), "{}").await?;
    assert_eq!(std::fs::read_to_string(path)?, "{}");
    Ok(())
}

#[tokio::test]
async fn policy_file_in_a_missing_directory_fails() -> Result<(), Box<dyn Error>> {
    let dir = tempfile::tempdir()?;
    let result = write_policy_file(&dir.path().join("absent"), "{}").await;
    assert!(matches!(result, Err(DockerError::PolicyWrite { .. })));
    Ok(())
}
