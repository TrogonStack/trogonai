use std::ffi::OsStr;

use crate::{adapters::EnvironmentVariable, session_prompt::js_truthy};

/// `u.bool()` upstream, which is `Te` of the variable.
pub(crate) fn flag_is_true<H: EnvironmentVariable>(host: &H, name: &str) -> bool {
    host.var_os(OsStr::new(name))
        .is_some_and(|value| js_truthy(&value.to_string_lossy()))
}

#[cfg(test)]
#[path = "settings_resolve_env_tests.rs"]
mod tests;
