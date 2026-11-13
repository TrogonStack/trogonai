#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::io;
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

use super::{
    CheckWritable, EffectiveGroupId, EffectiveUserId, HostIdentity, NativeHost, RealGroupId, RealUserId,
    SupplementaryGroupIds,
};
use crate::adapters::ReadMetadata;

#[test]
fn a_directory_the_process_made_passes_the_write_check() {
    let directory = tempfile::tempdir().expect("directory");

    NativeHost.check_writable(directory.path()).expect("writable");
}

#[test]
fn a_missing_path_fails_the_write_check_as_not_found() {
    let directory = tempfile::tempdir().expect("directory");

    let error = NativeHost
        .check_writable(&directory.path().join("missing"))
        .expect_err("missing");

    assert_eq!(error.kind(), io::ErrorKind::NotFound);
}

#[test]
fn the_native_username_is_never_empty() {
    assert!(NativeHost.username().is_none_or(|name| !name.is_empty()));
}

#[test]
fn metadata_follows_the_path_to_the_file() {
    let directory = tempfile::tempdir().expect("directory");
    let file = directory.path().join("settings.json");
    std::fs::write(&file, b"{}\n").expect("write");

    let metadata = NativeHost.metadata(&file).expect("metadata");

    assert!(metadata.is_file());
    assert_eq!(metadata.len(), 3);
}

#[cfg(unix)]
#[test]
fn a_new_file_belongs_to_the_effective_user() {
    let directory = tempfile::tempdir().expect("directory");
    let file = directory.path().join("owned");
    std::fs::write(&file, b"").expect("write");

    let owner = NativeHost.metadata(&file).expect("metadata").uid();

    assert_eq!(NativeHost.effective_user_id(), Some(owner));
}

#[cfg(unix)]
#[test]
fn every_process_id_call_answers_on_unix() {
    assert!(NativeHost.real_user_id().is_some());
    assert!(NativeHost.effective_group_id().is_some());
    assert!(NativeHost.real_group_id().is_some());
    NativeHost
        .supplementary_group_ids()
        .expect("getgroups exists")
        .expect("getgroups");
}

#[cfg(not(unix))]
#[test]
fn no_process_id_call_answers_off_unix() {
    assert!(NativeHost.effective_user_id().is_none());
    assert!(NativeHost.real_user_id().is_none());
    assert!(NativeHost.effective_group_id().is_none());
    assert!(NativeHost.real_group_id().is_none());
    assert!(NativeHost.supplementary_group_ids().is_none());
}
