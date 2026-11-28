//! A variable the daemon reads that is not valid Unicode is a configuration
//! error. Reading it as unset would drop the setting without a word, and for a
//! guard such as a termination requirement that loosens the proxy.

use std::ffi::OsString;

const PREFIX: &str = "EGRESS_PROXY_";

/// Refuses the first `EGRESS_PROXY_` variable whose value is not valid Unicode.
pub(super) fn reject_non_unicode(vars: impl IntoIterator<Item = (OsString, OsString)>) -> Result<(), String> {
    for (name, value) in vars {
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with(PREFIX) && value.to_str().is_none() {
            return Err(format!("{name} must be valid Unicode"));
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
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
}
