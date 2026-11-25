use std::ffi::OsStr;

use crate::{adapters::EnvironmentVariable, session_prompt::js_env_truthy};

/// `u.bool()` upstream, which is `Te` of the variable.
pub(crate) fn flag_is_true<H: EnvironmentVariable>(host: &H, name: &str) -> bool {
    js_env_truthy(host.var_os(OsStr::new(name)).as_deref())
}

#[cfg(test)]
#[path = "settings_resolve_env_tests.rs"]
mod tests;
