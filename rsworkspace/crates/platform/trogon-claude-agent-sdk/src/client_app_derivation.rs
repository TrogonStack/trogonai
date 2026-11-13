//! Python dialect derivation of `CLAUDE_AGENT_SDK_DERIVED_CLIENT_APP`, matching `_derive_client_app` (0.2.181).
//!
//! Python names the app from the installed distribution of the running program. This crate substitutes its own
//! package name and version, because Rust has no distribution metadata to look up.

use std::path::{Path, PathBuf};

use crate::{
    adapters::{NativeFileSystem, NativeHost},
    bundled_cli::bundled_cli_derives_client_app,
    client_app::format_client_app,
    constants::{SDK_PACKAGE_NAME, SDK_VERSION},
};

/// The Claude Code executable the application ships in a `_bundled` directory with a `claude.version` record,
/// Python's bundled CLI.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BundledCli {
    /// The directory that holds `_bundled`, Python's package directory. Only the executable that
    /// [`crate::find_bundled_cli`] finds there qualifies.
    pub package_root: PathBuf,
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

/// Python `_derive_client_app`: only the bundled CLI qualifies, and only when both the pin and the recorded
/// version meet the minimum. A failure to read the record is no label, never an error. The caller decides whether
/// an explicit label already exists, which Python checks before deriving.
pub(crate) fn derive(derivation: &ClientAppDerivation, cli_path: &Path) -> Option<String> {
    let ClientAppDerivation::Bundled(bundled) = derivation else {
        return None;
    };
    bundled_cli_derives_client_app(
        cli_path,
        &bundled.package_root,
        cfg!(windows),
        &bundled.pinned_version,
        &NativeFileSystem,
        &NativeHost,
    )
    .then(|| format_client_app(SDK_PACKAGE_NAME, Some(SDK_VERSION)))
    .flatten()
}

#[cfg(test)]
mod tests;
