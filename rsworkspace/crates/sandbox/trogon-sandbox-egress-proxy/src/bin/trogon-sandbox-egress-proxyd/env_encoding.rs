//! A variable the daemon reads that is not valid Unicode is a configuration
//! error. Reading it as unset would drop the setting without a word, and for a
//! guard such as a termination requirement that loosens the proxy.

use std::ffi::OsString;

use super::constants::ENV_VAR_PREFIX;

/// Refuses the first `EGRESS_PROXY_` variable whose value is not valid Unicode.
pub(super) fn reject_non_unicode(vars: impl IntoIterator<Item = (OsString, OsString)>) -> Result<(), String> {
    for (name, value) in vars {
        let Some(name) = name.to_str() else {
            continue;
        };
        if name.starts_with(ENV_VAR_PREFIX) && value.to_str().is_none() {
            return Err(format!("{name} must be valid Unicode"));
        }
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests;
