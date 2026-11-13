use std::ffi::OsString;
use std::path::PathBuf;

use crate::upstream::settings::Settings;

impl Settings {
    /// Decodes a settings document. Keys the pinned declarations do not list are kept.
    pub fn from_json(text: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(text)
    }

    /// Decodes a settings document held as a JSON value.
    pub fn from_value(value: serde_json::Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(value)
    }

    /// Encodes the compact JSON `JSON.stringify` would write, with the keys in the order they
    /// were decoded with.
    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    /// Encodes the settings as a JSON value.
    pub fn to_value(&self) -> Result<serde_json::Value, serde_json::Error> {
        serde_json::to_value(self)
    }
}

/// The three forms upstream accepts for the `--settings` value.
///
/// TypeScript `Options.settings` is `string | Settings` and Python `ClaudeAgentOptions.settings`
/// is a string holding a path or JSON text. A string cannot say which it is, so Rust names it:
/// the process receives the path or JSON text as-is, and an inline object is written as compact
/// JSON. Nothing is read or validated here; the CLI does that.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsInput {
    /// A settings file path, passed through unchanged.
    Path(PathBuf),
    /// JSON text, passed through unchanged.
    Json(String),
    /// A settings object, written as compact JSON.
    Inline(Box<Settings>),
}

impl SettingsInput {
    pub fn path(path: impl Into<PathBuf>) -> Self {
        Self::Path(path.into())
    }

    pub fn json(text: impl Into<String>) -> Self {
        Self::Json(text.into())
    }

    pub fn inline(settings: Settings) -> Self {
        Self::Inline(Box::new(settings))
    }

    /// Classifies a Python-style settings string: text that starts with `{` and ends with `}`
    /// after trimming is JSON, anything else is a path (Python `_build_settings_value`).
    pub fn from_wire_string(value: impl Into<String>) -> Self {
        let value = value.into();
        let trimmed = value.trim();
        if trimmed.starts_with('{') && trimmed.ends_with('}') {
            Self::Json(value)
        } else {
            Self::Path(PathBuf::from(value))
        }
    }

    /// The text of the `--settings` argument.
    pub fn to_wire_value(&self) -> Result<OsString, serde_json::Error> {
        match self {
            Self::Path(path) => Ok(path.clone().into_os_string()),
            Self::Json(text) => Ok(OsString::from(text)),
            Self::Inline(settings) => settings.to_json().map(OsString::from),
        }
    }
}

impl From<Settings> for SettingsInput {
    fn from(settings: Settings) -> Self {
        Self::inline(settings)
    }
}
