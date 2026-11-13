use super::{home_folder, lookup, lookup_error, passwd_entry};
use crate::constants::PASSWD_MAX_BUFFER;
use std::{cell::Cell, io};

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

#[test]
fn absent_name_statuses_read_as_not_found_and_others_keep_their_code() {
    let not_found = || io::Error::other("absent");
    for code in [libc::ENOENT, libc::ESRCH, libc::EBADF, libc::EPERM] {
        assert_eq!(lookup_error(code, not_found).to_string(), "absent");
    }
    assert_eq!(lookup_error(libc::EIO, not_found).raw_os_error(), Some(libc::EIO));
}

#[test]
fn the_effective_user_has_a_home_folder_in_the_password_database() -> Result<(), io::Error> {
    let home = home_folder()?;
    assert!(home.is_absolute(), "{}", home.display());
    Ok(())
}

#[test]
fn a_query_that_finds_nothing_reads_as_none() {
    assert_eq!(passwd_entry(8, |_, _, _| 0, |record| record.pw_uid), Ok(None));
}

#[test]
fn a_query_status_other_than_erange_is_returned() {
    assert_eq!(
        passwd_entry(8, |_, _, _| libc::EIO, |record| record.pw_uid),
        Err(libc::EIO)
    );
}

#[test]
fn the_buffer_doubles_while_the_query_reports_erange() {
    let entry = passwd_entry(
        1,
        |record, buffer, found| {
            if buffer.len() < 16 {
                return libc::ERANGE;
            }
            // SAFETY: every field of `passwd` is an integer or a pointer, for which all zero bytes are valid, and
            // `record` and `found` point at locals `passwd_entry` owns.
            unsafe {
                let mut entry: libc::passwd = std::mem::zeroed();
                entry.pw_uid = u32::try_from(buffer.len()).unwrap_or(u32::MAX);
                record.write(entry);
                found.write(record);
            }
            0
        },
        |record| record.pw_uid,
    );
    assert_eq!(entry, Ok(Some(16)));
}

#[test]
fn the_buffer_stops_growing_at_the_cap() {
    let largest = Cell::new(0);
    let entry = passwd_entry(
        1,
        |_, buffer, _| {
            largest.set(buffer.len());
            libc::ERANGE
        },
        |record| record.pw_uid,
    );
    assert_eq!(entry, Err(libc::ERANGE));
    assert_eq!(largest.get(), PASSWD_MAX_BUFFER);
}
