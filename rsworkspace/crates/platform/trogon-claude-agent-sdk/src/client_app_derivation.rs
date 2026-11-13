//! Python dialect derivation of `CLAUDE_AGENT_SDK_DERIVED_CLIENT_APP`, matching `_derive_client_app` (0.2.181).
//!
//! Python names the app from the installed distribution of the running program. This crate substitutes its own
//! package name and version, because Rust has no distribution metadata to look up.

use std::path::{Path, PathBuf};

use crate::{
    client_app::format_client_app,
    constants::{BUNDLED_CLI_VERSION_FILE, DERIVED_CLIENT_APP_MINIMUM_VERSION, SDK_PACKAGE_NAME, SDK_VERSION},
};

/// A Claude Code executable the application ships beside a `claude.version` record, Python's `_bundled` CLI.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BundledCli {
    /// Where the bundled executable lives. The version record is the sibling file `claude.version`.
    pub path: PathBuf,
    /// Python `__cli_version__`, the version of the CLI this build bundles. Below the minimum nothing reads the disk.
    pub pinned_version: String,
}

/// Python dialect: whether `CLAUDE_AGENT_SDK_DERIVED_CLIENT_APP` is derived. TypeScript never derives one, so
/// the default is no derivation.
///
/// Python names the app from the installed distribution of the running program. This crate substitutes its own
/// package name and version, because Rust has no distribution metadata to look up.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum ClientAppDerivation {
    #[default]
    Disabled,
    Bundled(BundledCli),
}

/// Python `_meets_derived_client_app_minimum`: `re.match(r"(\d{1,9})\.(\d{1,9})\.(\d{1,9})")` then a numeric compare.
pub(crate) fn meets_derived_minimum(version: &str) -> bool {
    fn number(text: &str) -> Option<(u64, &str)> {
        let end = text.find(|c: char| !c.is_ascii_digit()).unwrap_or(text.len());
        if end == 0 || end > 9 {
            return None;
        }
        Some((text[..end].parse().ok()?, &text[end..]))
    }
    let parsed = number(version)
        .and_then(|(major, rest)| Some((major, number(rest.strip_prefix('.')?)?)))
        .and_then(|(major, (minor, rest))| Some((major, minor, number(rest.strip_prefix('.')?)?.0)));
    parsed.is_some_and(|(major, minor, patch)| [major, minor, patch] >= DERIVED_CLIENT_APP_MINIMUM_VERSION)
}

/// Python `_derive_client_app`: only the bundled CLI qualifies, and only when both the pin and the recorded
/// version meet the minimum. A failure to read the record is no label, never an error. The caller decides whether
/// an explicit label already exists, which Python checks before deriving.
pub(crate) fn derive(derivation: &ClientAppDerivation, cli_path: &Path) -> Option<String> {
    let ClientAppDerivation::Bundled(bundled) = derivation else {
        return None;
    };
    if !meets_derived_minimum(&bundled.pinned_version) || cli_path != bundled.path {
        return None;
    }
    let record = std::fs::read_to_string(bundled.path.with_file_name(BUNDLED_CLI_VERSION_FILE)).ok()?;
    if !meets_derived_minimum(&record) {
        return None;
    }
    format_client_app(SDK_PACKAGE_NAME, Some(SDK_VERSION))
}

#[cfg(test)]
mod tests;
