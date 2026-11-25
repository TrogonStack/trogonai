#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::{fs, io};

use super::{Bridge, errno, libuv_final_path, locate_document, node_code, node_error, os_code, platform, windows_code};
use crate::{
    adapters::{NativeHost, OperatingSystem},
    connector_vetting::{Located, Platform, VettingHost},
    node_path::{NodeProcess, js},
    settings_resolve_state::ResolverState,
};

const MISSING: &str = "/trogon-claude-sdk-no-such-folder";

fn code(text: &str) -> Option<String> {
    Some(text.to_owned())
}

#[test]
fn a_windows_final_path_reads_the_way_libuv_shortens_it() {
    let lone: Vec<u16> = [js("\\\\?\\C:\\a"), vec![0xD800], js("b")].concat();
    let pair: Vec<u16> = [js("\\\\?\\C:\\"), "\u{1F600}".encode_utf16().collect()].concat();

    assert_eq!(libuv_final_path(&js("\\\\?\\C:\\work")), Ok(js("C:\\work")));
    assert_eq!(
        libuv_final_path(&js("\\\\?\\UNC\\server\\share\\x")),
        Ok(js("\\\\server\\share\\x"))
    );
    assert_eq!(libuv_final_path(&js("\\\\?\\Volume{1}\\x")), Ok(js("Volume{1}\\x")));
    assert_eq!(libuv_final_path(&js("C:\\work")), Err(code("EBADF")));
    assert_eq!(libuv_final_path(&lone), Ok(js("C:\\a\u{FFFD}\u{FFFD}\u{FFFD}b")));
    assert_eq!(
        libuv_final_path(&pair),
        Ok([js("C:\\"), "\u{1F600}".encode_utf16().collect()].concat())
    );
}

#[test]
fn windows_system_errors_take_the_code_libuv_translates_them_to() {
    for (raw, name) in [
        (2, "ENOENT"),
        (3, "ENOENT"),
        (123, "ENOENT"),
        (267, "ENOENT"),
        (5, "EPERM"),
        (1920, "EACCES"),
        (206, "ENAMETOOLONG"),
        (1921, "ELOOP"),
        (1, "EISDIR"),
        (32, "EBUSY"),
        (87, "EINVAL"),
        (4390, "UNKNOWN"),
    ] {
        assert_eq!(windows_code(raw), name, "{raw}");
    }
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

#[test]
fn the_bridge_names_where_a_path_really_is() {
    let bridge = Bridge { host: &NativeHost };
    assert_eq!(bridge.real_path(&js("/")), Ok(js("/")));
    assert_eq!(bridge.real_path(&js(MISSING)), Err(code("ENOENT")));
}

#[test]
fn a_managed_document_is_located_with_the_folder_it_was_read_from() {
    let root = tempfile::tempdir().unwrap();
    let folder = root.path().canonicalize().unwrap();
    fs::write(folder.join("managed-settings.json"), "{}").unwrap();
    let state = ResolverState::new();
    let real = |path: &std::path::Path| Located::Real(js(path.to_str().unwrap()));
    assert_eq!(
        locate_document(&NativeHost, &state, &folder, &["managed-settings.json".to_owned()]),
        (real(&folder.join("managed-settings.json")), real(&folder))
    );
    assert_eq!(
        locate_document(&NativeHost, &state, &folder, &["missing.json".to_owned()]),
        (Located::Gone, real(&folder))
    );
}
