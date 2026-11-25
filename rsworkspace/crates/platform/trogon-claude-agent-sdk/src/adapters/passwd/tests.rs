use super::*;

#[test]
fn root_has_a_home_directory() {
    assert!(home_of_uid(0).is_some());
}

#[test]
fn unknown_user_has_no_home_directory() {
    assert_eq!(home_of_uid(4_000_000_000), None);
}

#[test]
fn a_small_buffer_grows_until_the_record_fits() {
    assert_eq!(home_of_uid_with_buffer(0, 1, PASSWD_MAX_BUFFER), home_of_uid(0));
}

#[test]
fn a_buffer_at_its_limit_gives_up() {
    assert_eq!(home_of_uid_with_buffer(0, 1, 1), None);
}

#[test]
fn effective_and_real_homes_are_those_of_their_user_ids() {
    // SAFETY: `geteuid` and `getuid` take no arguments and always succeed.
    let (effective, real) = unsafe { (libc::geteuid(), libc::getuid()) };
    assert_eq!(effective_user_home(), home_of_uid(effective));
    assert_eq!(real_user_home(), home_of_uid(real));
}
