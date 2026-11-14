use std::{
    io,
    path::{Component, Path, PathBuf},
};

use unicode_normalization::UnicodeNormalization;

use crate::{
    adapters::{CurrentDirectory, EnvironmentVariable, ReadFile},
    constants::{LOCAL_SETTINGS_FILE, PROJECT_SETTINGS_FILE, USER_SETTINGS_FILE},
    home_directory::{HomeDirectoryError, Platform, node_homedir, node_join},
    node_text_file::read_node_text,
    python_text_file::read_text_utf8_sig,
    settings_dialect::SettingsDialect,
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

/// The directory `userSettings` lives in: `CLAUDE_CONFIG_DIR` as set (an empty value counts), else
/// `path.join(os.homedir(), ".claude")` (`Wt` upstream), which throws when no home directory exists.
pub(crate) fn config_directory<H: EnvironmentVariable>(host: &H) -> Result<PathBuf, HomeDirectoryError> {
    let platform = Platform::host();
    let chosen = match host.var_os(std::ffi::OsStr::new("CLAUDE_CONFIG_DIR")) {
        Some(directory) => PathBuf::from(directory),
        None => node_join(platform, &node_homedir(host, platform)?, ".claude"),
    };
    Ok(nfc(&chosen))
}

pub(crate) fn user_settings_path<H: EnvironmentVariable>(host: &H) -> Result<PathBuf, HomeDirectoryError> {
    let directory = config_directory(host)?;
    Ok(node_join(Platform::host(), directory.as_os_str(), USER_SETTINGS_FILE))
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

/// The text of one settings file, `None` when it is missing, unreadable or cannot be decoded. The TypeScript dialect
/// decodes the bytes as Node hands them to the TypeScript settings reader ([`read_node_text`]). The Python dialect
/// reads them as the Python SDK reads its one settings file, with `Path.read_text(encoding="utf-8-sig")`, and a file
/// that read raises on yields nothing, as a file `json.loads` rejects does: the Python SDK has no `resolveSettings`,
/// and `resolveSettings` drops every settings file it cannot read or parse. Only the `--settings` value the Python
/// dialect builds raises on such a file, with the [`SettingsLoadError`](crate::SettingsLoadError) Python raises.
pub(crate) fn read_settings_text<H: ReadFile<Error = io::Error>>(
    host: &H,
    path: &Path,
    dialect: SettingsDialect,
) -> Option<String> {
    let bytes = host.read_file(path).ok()?;
    match dialect {
        SettingsDialect::TypeScript => Some(read_node_text(&bytes)),
        SettingsDialect::Python => read_text_utf8_sig(&bytes).ok(),
    }
}
