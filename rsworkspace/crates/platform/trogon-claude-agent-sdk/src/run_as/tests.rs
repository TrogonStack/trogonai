use super::lookup;

#[test]
fn the_superuser_resolves_through_the_system_user_database() -> Result<(), std::io::Error> {
    assert_eq!(lookup("root")?, 0);
    Ok(())
}

#[test]
fn an_unknown_user_reports_the_getpwnam_text() {
    let error = lookup("trogon-no-such-user-x").err();
    let message = error.as_ref().map(ToString::to_string);
    assert_eq!(
        message.as_deref(),
        Some("getpwnam(): name not found: 'trogon-no-such-user-x'")
    );
    assert_eq!(error.map(|e| e.kind()), Some(std::io::ErrorKind::NotFound));
}

#[test]
fn a_name_with_a_nul_byte_is_refused() {
    assert_eq!(
        lookup("ro\0ot").err().map(|e| e.kind()),
        Some(std::io::ErrorKind::InvalidInput)
    );
}

#[test]
fn setting_the_uid_to_the_current_one_needs_no_privilege() -> Result<(), std::io::Error> {
    // SAFETY: `getuid` takes no arguments and cannot fail.
    let uid = unsafe { libc::getuid() };
    super::set_real_and_effective_uid(uid)
}

#[test]
fn changing_to_another_uid_without_privilege_reports_the_os_error() {
    // SAFETY: `getuid` takes no arguments and cannot fail.
    let uid = unsafe { libc::getuid() };
    if uid == 0 {
        return;
    }
    let error = super::set_real_and_effective_uid(uid.wrapping_add(1)).err();
    assert_eq!(error.map(|e| e.kind()), Some(std::io::ErrorKind::PermissionDenied));
}
