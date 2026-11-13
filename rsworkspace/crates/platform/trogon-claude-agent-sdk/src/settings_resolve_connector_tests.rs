#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::io;

use super::{Bridge, errno, node_code, node_error, os_code, platform};
use crate::{
    adapters::{NativeHost, OperatingSystem},
    connector_vetting::{Platform, VettingHost},
    node_path::{NodeProcess, js},
};

const MISSING: &str = "/trogon-claude-sdk-no-such-folder";

fn code(text: &str) -> Option<String> {
    Some(text.to_owned())
}

#[test]
fn raw_os_errors_take_the_libuv_code_name() {
    for (raw, name) in [
        (libc::ENOENT, "ENOENT"),
        (libc::ENOTDIR, "ENOTDIR"),
        (libc::EACCES, "EACCES"),
        (libc::EPERM, "EPERM"),
        (libc::ENAMETOOLONG, "ENAMETOOLONG"),
        (libc::ELOOP, "ELOOP"),
        (libc::EISDIR, "EISDIR"),
        (libc::EINVAL, "EINVAL"),
        (libc::EIO, "EIO"),
        (libc::EBUSY, "EBUSY"),
    ] {
        assert_eq!(os_code(raw), Some(name));
        assert_eq!(node_code(&io::Error::from_raw_os_error(raw)), code(name));
    }
    assert_eq!(os_code(libc::EEXIST), None);
}

#[test]
fn errors_without_a_known_os_code_are_named_by_their_kind() {
    for (kind, name) in [
        (io::ErrorKind::NotFound, "ENOENT"),
        (io::ErrorKind::PermissionDenied, "EACCES"),
        (io::ErrorKind::NotADirectory, "ENOTDIR"),
        (io::ErrorKind::IsADirectory, "EISDIR"),
        (io::ErrorKind::InvalidFilename, "ENAMETOOLONG"),
        (io::ErrorKind::InvalidInput, "EINVAL"),
        (io::ErrorKind::TimedOut, "UNKNOWN"),
    ] {
        assert_eq!(node_code(&io::Error::from(kind)), code(name), "{kind:?}");
    }
    assert_eq!(errno(io::Error::from_raw_os_error(libc::EEXIST)), code("UNKNOWN"));
}

#[test]
fn an_unsupported_probe_throws_without_a_code() {
    assert_eq!(node_code(&io::Error::from(io::ErrorKind::Unsupported)), None);
    assert_eq!(node_error(io::Error::from(io::ErrorKind::Unsupported)).code, None);
    assert_eq!(
        node_error(io::Error::from(io::ErrorKind::NotFound)).code,
        code("ENOENT")
    );
}

#[test]
fn the_platform_is_the_one_process_platform_names() {
    assert_eq!(platform(OperatingSystem::Darwin), Platform::Darwin);
    assert_eq!(platform(OperatingSystem::Win32), Platform::Win32);
    assert_eq!(platform(OperatingSystem::Linux), Platform::Other);
    assert_eq!(platform(OperatingSystem::Other("freebsd".to_owned())), Platform::Other);
}

#[test]
fn the_bridge_reads_the_native_host_the_way_node_does() {
    let bridge = Bridge { host: &NativeHost };
    assert_eq!(bridge.open_dir_no_follow(&js("/")), Ok(()));
    assert_eq!(bridge.open_dir_no_follow(&js(MISSING)), Err(code("ENOENT")));
    assert!(bridge.stat_device("/").is_ok());
    assert_eq!(bridge.stat_device(MISSING), Err(code("ENOENT")));
    let auto_master = bridge.read_auto_master();
    assert!(
        auto_master.is_ok() || auto_master == Err(code("ENOENT")),
        "{auto_master:?}"
    );
    assert!(!bridge.home_directory().unwrap().is_empty());
    assert!(!bridge.cwd().unwrap().is_empty());
}
