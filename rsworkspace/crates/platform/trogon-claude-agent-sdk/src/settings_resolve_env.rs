use std::ffi::OsStr;

use crate::adapters::EnvironmentVariable;

/// `Te` upstream: whether a variable holds `1`, `true`, `yes` or `on`, ignoring case and surrounding blanks.
pub(crate) fn flag_is_true<H: EnvironmentVariable>(host: &H, name: &str) -> bool {
    host.var_os(OsStr::new(name)).is_some_and(|value| {
        let text = value.to_string_lossy().trim().to_lowercase();
        matches!(text.as_str(), "1" | "true" | "yes" | "on")
    })
}
