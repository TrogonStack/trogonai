use std::{
    io,
    path::{Component, Path, PathBuf},
};

use serde_json::Value;
use unicode_normalization::UnicodeNormalization;

use crate::{
    adapters::{CurrentDirectory, EnvironmentVariable, ReadFile},
    constants::{LOCAL_SETTINGS_FILE, PROJECT_SETTINGS_FILE, USER_SETTINGS_FILE},
    settings_resolve_values::SettingsMap,
    settings_schema::schema,
};

/// `path.resolve` for one segment: absolute as is, relative against `base`, then lexically normalized.
pub(crate) fn resolve_path(base: &Path, path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    };
    let mut resolved = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                if resolved.parent().is_some() {
                    resolved.pop();
                }
            }
            other => resolved.push(other.as_os_str()),
        }
    }
    resolved
}

/// `String.prototype.normalize("NFC")` on a path.
pub(crate) fn nfc(path: &Path) -> PathBuf {
    PathBuf::from(path.to_string_lossy().nfc().collect::<String>())
}

fn home_directory<H: EnvironmentVariable>(host: &H) -> Option<PathBuf> {
    ["HOME", "USERPROFILE"]
        .iter()
        .find_map(|name| host.var_os(std::ffi::OsStr::new(name)))
        .map(PathBuf::from)
}

/// The directory `userSettings` lives in: `CLAUDE_CONFIG_DIR`, else `~/.claude` (`Wt` upstream).
pub(crate) fn config_directory<H: EnvironmentVariable>(host: &H) -> PathBuf {
    let chosen = host
        .var_os(std::ffi::OsStr::new("CLAUDE_CONFIG_DIR"))
        .map(PathBuf::from)
        .or_else(|| home_directory(host).map(|home| home.join(".claude")))
        .unwrap_or_else(|| PathBuf::from(".claude"));
    nfc(&chosen)
}

pub(crate) fn user_settings_path<H: EnvironmentVariable>(host: &H) -> PathBuf {
    nfc(&config_directory(host)).join(USER_SETTINGS_FILE)
}

pub(crate) fn project_settings_path(cwd: &Path) -> PathBuf {
    PROJECT_SETTINGS_FILE
        .iter()
        .fold(nfc(cwd), |path, part| path.join(part))
}

pub(crate) fn local_settings_path(cwd: &Path) -> PathBuf {
    LOCAL_SETTINGS_FILE.iter().fold(nfc(cwd), |path, part| path.join(part))
}

/// The working directory the cascade resolves against (`Eet(e.cwd ?? cwd())` upstream).
pub(crate) fn resolve_cwd<H: CurrentDirectory<Error = io::Error>>(
    host: &H,
    cwd: Option<&str>,
) -> Result<PathBuf, io::Error> {
    let process_directory = host.current_dir()?;
    Ok(match cwd {
        Some(cwd) => resolve_path(&process_directory, Path::new(cwd)),
        None => resolve_path(&process_directory, &process_directory),
    })
}

/// Reads one settings file; a missing, unreadable, non-object or invalid file yields nothing.
pub(crate) fn read_settings_file<H: ReadFile<Error = io::Error>>(host: &H, path: &Path) -> Option<SettingsMap> {
    let bytes = host.read_file(path).ok()?;
    let text = String::from_utf8_lossy(&bytes);
    if is_blank(&text) {
        return Some(SettingsMap::new());
    }
    match serde_json::from_str::<Value>(text.strip_prefix('\u{feff}').unwrap_or(&text)).ok()? {
        Value::Object(map) => schema()?.parse_document(&map),
        _ => None,
    }
}

/// `String.prototype.trim() === ""`: JavaScript counts the byte order mark as white space.
fn is_blank(text: &str) -> bool {
    text.chars().all(|c| c.is_whitespace() || c == '\u{feff}')
}
