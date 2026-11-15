//! The TypeScript SDK's default debug file (`uq` in `sdk.mjs`): when `DEBUG_CLAUDE_AGENT_SDK` is truthy, the
//! first connection of the process picks `<config dir>/debug/sdk-<uuid>.txt`, announces it on stderr, creates the
//! directory and passes it to the CLI as `--debug-file`. Every later connection reuses the same path.

use std::{ffi::OsStr, io::Write, path::PathBuf, sync::OnceLock};

use unicode_normalization::UnicodeNormalization;

use crate::{
    adapters::EnvironmentVariable,
    constants::SDK_DEBUG_ENV,
    environment_plan::Inherited,
    home_directory::{HomeDirectoryError, Platform, node_homedir, node_join},
};

static DERIVED: OnceLock<Option<PathBuf>> = OnceLock::new();

/// JavaScript `Te`: `1`, `true`, `yes` or `on`, ignoring case and surrounding whitespace.
fn truthy(value: Option<&std::ffi::OsString>) -> bool {
    value.is_some_and(|value| {
        matches!(
            value.to_string_lossy().trim().to_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

/// The configuration directory, `Wt()`: `CLAUDE_CONFIG_DIR` when set (even empty), otherwise
/// `path.join(os.homedir(), ".claude")`, normalized to NFC. `os.homedir()` reads the process environment, so the
/// home comes from `environment`, and a missing home is an error exactly where Node throws.
fn config_dir<E: EnvironmentVariable>(environment: &E, inherited: &Inherited) -> Result<PathBuf, HomeDirectoryError> {
    let platform = Platform::host();
    let directory = match inherited.get(OsStr::new("CLAUDE_CONFIG_DIR")) {
        Some(directory) => PathBuf::from(directory),
        None => node_join(platform, &node_homedir(environment, platform)?, ".claude"),
    };
    Ok(PathBuf::from(directory.to_string_lossy().nfc().collect::<String>()))
}

/// The path for one session id, or `None` when the debug variable is not truthy.
pub(crate) fn path_for<E: EnvironmentVariable>(
    environment: &E,
    inherited: &Inherited,
    id: uuid::Uuid,
) -> Result<Option<PathBuf>, HomeDirectoryError> {
    if !truthy(inherited.get(OsStr::new(SDK_DEBUG_ENV))) {
        return Ok(None);
    }
    let platform = Platform::host();
    let directory = node_join(platform, config_dir(environment, inherited)?.as_os_str(), "debug");
    Ok(Some(node_join(
        platform,
        directory.as_os_str(),
        &format!("sdk-{id}.txt"),
    )))
}

/// The process-wide default debug file. The first successful call decides; the notice and the directory creation
/// happen once, and a failure to create the directory is ignored, as upstream does. A missing home directory is an
/// error and decides nothing, as the throw in the upstream constructor does.
pub(crate) fn process_debug_file<E: EnvironmentVariable>(
    environment: &E,
    inherited: &Inherited,
) -> Result<Option<PathBuf>, HomeDirectoryError> {
    if let Some(decided) = DERIVED.get() {
        return Ok(decided.clone());
    }
    let path = path_for(environment, inherited, uuid::Uuid::new_v4())?;
    Ok(DERIVED
        .get_or_init(|| {
            if let Some(path) = &path {
                let _ = writeln!(std::io::stderr(), "SDK debug logs: {}", path.display());
                if let Some(directory) = path.parent() {
                    let _ = std::fs::create_dir_all(directory);
                }
            }
            path
        })
        .clone())
}

#[cfg(test)]
mod tests;
