use std::fs;
use std::io;

use super::*;

#[cfg(unix)]
#[test]
fn tells_apart_what_a_path_is_on_the_real_host() -> io::Result<()> {
    let temp = tempfile::tempdir()?;
    let root = temp.path();
    fs::write(root.join("file"), "x")?;
    fs::create_dir(root.join("dir"))?;
    std::os::unix::fs::symlink(root.join("dir"), root.join("link"))?;
    std::os::unix::fs::symlink(root.join("gone"), root.join("dangling"))?;
    std::os::unix::fs::symlink("a\u{fffd}", root.join("odd"))?;
    let name = |entry: &str| root.join(entry).to_string_lossy().into_owned();
    let probe = HostProbe;
    assert_eq!(probe.entry_kind(&name("file")), EntryKind::File);
    assert_eq!(probe.entry_kind(&name("dir")), EntryKind::Directory);
    assert_eq!(probe.entry_kind(&name("link")), EntryKind::Symlink);
    assert_eq!(probe.entry_kind(&name("missing")), EntryKind::Absent);
    assert_eq!(probe.entry_kind(&name("file/below")), EntryKind::Absent);
    assert_eq!(probe.followed_kind(&name("link")), EntryKind::Directory);
    assert_eq!(probe.followed_kind(&name("dangling")), EntryKind::Absent);
    assert_eq!(probe.entry_kind("/dev/null"), EntryKind::Special);
    assert_eq!(probe.entry_kind(&format!("{}/\0", root.display())), EntryKind::Other);
    assert_eq!(probe.link_text(&name("link")), Some(name("dir")));
    assert_eq!(probe.link_text(&name("file")), None);
    assert_eq!(probe.link_text(&name("odd")).as_deref(), Some("a\u{fffd}"));
    assert_eq!(probe.read_text(&name("file")), Ok("x".to_owned()));
    assert_eq!(probe.read_text(&name("missing")), Err(ProbeError::coded("ENOENT")));
    assert_eq!(probe.read_text(&name("file/below")), Err(ProbeError::coded("ENOTDIR")));
    assert_eq!(probe.device_of(&name("dir")), probe.device_of(&name("file")));
    assert_eq!(probe.device_of(&name("missing")), Err(ProbeError::coded("ENOENT")));
    Ok(())
}

#[cfg(unix)]
#[test]
fn reads_the_file_system_type_and_names_the_failures() -> io::Result<()> {
    let probe = HostProbe;
    assert!(probe.statfs_type("/").is_ok());
    assert_eq!(
        probe.statfs_type("/nonexistent-directory-for-statfs"),
        Err(ProbeError::coded("ENOENT"))
    );
    assert_eq!(
        probe.statfs_type("a\0b"),
        Err(ProbeError::coded("ERR_INVALID_ARG_VALUE"))
    );
    for (errno, code) in [
        (libc::ENOENT, "ENOENT"),
        (libc::ENOTDIR, "ENOTDIR"),
        (libc::EACCES, "EACCES"),
        (libc::EPERM, "EPERM"),
        (libc::ENOSYS, "ENOSYS"),
        (libc::ENOTSUP, "ENOTSUP"),
        (libc::EOPNOTSUPP, "ENOTSUP"),
    ] {
        let named = probe_error(&io::Error::from_raw_os_error(errno));
        assert!(named.code.is_some(), "{code}");
        assert!(!named.type_error);
    }
    assert_eq!(
        probe_error(&io::Error::from_raw_os_error(libc::EIO)).code,
        Some(format!("E{}", libc::EIO))
    );
    assert_eq!(probe_error(&io::Error::other("no errno")).code, None);
    Ok(())
}

#[test]
fn reports_the_platform_and_the_home_directory() {
    let probe = HostProbe;
    let platform = probe.platform();
    assert_eq!(platform == Platform::Darwin, cfg!(target_os = "macos"));
    assert!(matches!(
        probe.runtime_platform(),
        Platform::Linux | Platform::Wsl | Platform::Darwin | Platform::Win32 | Platform::Other
    ));
    if platform != Platform::Linux {
        assert_eq!(probe.runtime_platform(), platform);
    }
    let variable = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    assert_eq!(
        probe.home_dir(),
        crate::adapters::EnvironmentVariable::var_os(
            &crate::adapters::SystemEnvironment,
            std::ffi::OsStr::new(variable)
        )
        .and_then(|home| home.into_string().ok())
        .filter(|home| !home.is_empty())
    );
}
