use std::path::{Path, PathBuf};

use trogon_sandbox_policy_exec::{HostPaths, SandboxSettings, SettingsFile};

/// Where the settings come from. A file that was asked for must load; only
/// an absent default file falls back to the built-in settings.
#[derive(Debug, Eq, PartialEq)]
pub enum SettingsSource {
    Requested(PathBuf),
    Default(PathBuf),
    NoHome,
}

impl SettingsSource {
    pub fn choose(requested: Option<PathBuf>, home: Option<PathBuf>) -> Self {
        match (requested, home) {
            (Some(path), _) => Self::Requested(path),
            (None, Some(home)) => Self::Default(home.join(DEFAULT_FILE_NAME)),
            (None, None) => Self::NoHome,
        }
    }

    pub fn load(&self, host: Option<&HostPaths>) -> Result<SandboxSettings, Refusal> {
        match self {
            Self::NoHome => built_in(host),
            Self::Default(path) => match read_file(path, host) {
                SettingsFile::Missing => built_in(host),
                file => Self::settings(file, path, FILE_RULES),
            },
            Self::Requested(path) => match read_file(path, host) {
                SettingsFile::Missing => Self::settings(SettingsFile::Missing, path, REQUESTED_RULES),
                file => Self::settings(file, path, FILE_RULES),
            },
        }
    }

    fn settings(file: SettingsFile, path: &Path, dropped: &str) -> Result<SandboxSettings, Refusal> {
        match file {
            SettingsFile::Loaded(settings) => Ok(*settings),
            other => Err(Refusal {
                reason: other
                    .problem(path)
                    .unwrap_or_else(|| format!("{} could not be used.", path.display())),
                dropped: dropped.to_string(),
            }),
        }
    }
}

fn read_file(path: &Path, host: Option<&HostPaths>) -> SettingsFile {
    SettingsFile::read_lenient(path, host)
}

fn built_in(host: Option<&HostPaths>) -> Result<SandboxSettings, Refusal> {
    match host {
        Some(host) => SandboxSettings::from_json_for_host(BUILT_IN, host),
        None => SandboxSettings::from_json(BUILT_IN),
    }
    .map_err(|error| Refusal {
        reason: format!("The built-in settings are not valid: {error}"),
        dropped: "the built-in settings".to_string(),
    })
}

#[derive(Debug, Eq, PartialEq)]
pub struct Refusal {
    reason: String,
    dropped: String,
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}\nRefusing to run with the built-in defaults, which would drop {}.",
            self.reason, self.dropped
        )
    }
}

mod constants;
#[cfg(test)]
mod tests;
use constants::{BUILT_IN, DEFAULT_FILE_NAME, FILE_RULES, REQUESTED_RULES};
