//! The TypeScript SDK's default debug file (`uq` in `sdk.mjs`): when `DEBUG_CLAUDE_AGENT_SDK` is truthy, the
//! first connection of the process picks `<config dir>/debug/sdk-<uuid>.txt`, announces it on stderr, creates the
//! directory and passes it to the CLI as `--debug-file`. Every later connection reuses the same path.

use std::{
    ffi::OsStr,
    io::Write,
    path::{Path, PathBuf},
    sync::OnceLock,
};

use unicode_normalization::UnicodeNormalization;

use crate::{constants::SDK_DEBUG_ENV, environment_plan::Inherited};

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

/// The configuration directory, `Wt()`: `CLAUDE_CONFIG_DIR` when set (even empty), otherwise `~/.claude`,
/// normalized to NFC.
fn config_dir(inherited: &Inherited) -> Option<PathBuf> {
    let directory = match inherited.get(OsStr::new("CLAUDE_CONFIG_DIR")) {
        Some(directory) => PathBuf::from(directory),
        None => {
            let home = inherited.get(OsStr::new(if cfg!(windows) { "USERPROFILE" } else { "HOME" }))?;
            Path::new(home).join(".claude")
        }
    };
    Some(PathBuf::from(directory.to_string_lossy().nfc().collect::<String>()))
}

/// The path for one session id, or `None` when the debug variable is not truthy.
pub(crate) fn path_for(inherited: &Inherited, id: uuid::Uuid) -> Option<PathBuf> {
    if !truthy(inherited.get(OsStr::new(SDK_DEBUG_ENV))) {
        return None;
    }
    Some(config_dir(inherited)?.join("debug").join(format!("sdk-{id}.txt")))
}

/// The process-wide default debug file. The first call decides; the notice and the directory creation happen
/// once, and a failure to create the directory is ignored, as upstream does.
pub(crate) fn process_debug_file(inherited: &Inherited) -> Option<PathBuf> {
    DERIVED
        .get_or_init(|| {
            let path = path_for(inherited, uuid::Uuid::new_v4())?;
            let _ = writeln!(std::io::stderr(), "SDK debug logs: {}", path.display());
            if let Some(directory) = path.parent() {
                let _ = std::fs::create_dir_all(directory);
            }
            Some(path)
        })
        .clone()
}

#[cfg(test)]
mod tests;
