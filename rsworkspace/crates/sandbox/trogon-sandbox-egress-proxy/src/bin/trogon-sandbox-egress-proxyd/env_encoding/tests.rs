use std::os::unix::ffi::OsStringExt as _;

use super::*;

fn var(name: &str, value: Vec<u8>) -> (OsString, OsString) {
    (OsString::from(name), OsString::from_vec(value))
}

#[test]
fn a_non_unicode_proxy_variable_is_refused_by_name() {
    let vars = [var("EGRESS_PROXY_REQUIRE_TERMINATION_FOR_NAME_ONLY", vec![0x66, 0xff])];
    assert_eq!(
        reject_non_unicode(vars),
        Err("EGRESS_PROXY_REQUIRE_TERMINATION_FOR_NAME_ONLY must be valid Unicode".to_string())
    );
}

#[test]
fn unicode_proxy_variables_and_unrelated_variables_pass() {
    let vars = [
        var("EGRESS_PROXY_POLICY_FILE", b"/etc/policy.json".to_vec()),
        var("PATH_ELSEWHERE", vec![0xff]),
    ];
    assert_eq!(reject_non_unicode(vars), Ok(()));
}
