//! Python `CLIConnectionError("Failed to load settings: ...")`.

use crate::{
    PythonUnicodeDecodeError,
    python_json_error::{ExactIntegerError, JsonDecodeError},
};

/// The exception Python `_build_settings_value` caught and raised again: the `__cause__` of its
/// `CLIConnectionError`. `Display` is `str()` of that exception.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SettingsLoadCauseError {
    /// An `OSError` opening or reading the settings file, holding its `str()`.
    #[error("{0}")]
    Os(String),
    /// A `UnicodeDecodeError`: the settings file is not UTF-8.
    #[error(transparent)]
    UnicodeDecode(PythonUnicodeDecodeError),
    /// A `json.JSONDecodeError`: `json.loads` rejected the settings text.
    #[error(transparent)]
    JsonDecode(JsonDecodeError),
    /// A `ValueError` from `json.loads`: an integer in the settings has more digits than Python converts.
    #[error(transparent)]
    Integer(ExactIntegerError),
    /// A `TypeError`: the settings hold no object, so the sandbox cannot be set in them. It holds the `str()` of
    /// the exception.
    #[error("{0}")]
    Type(String),
}

/// Python `CLIConnectionError("Failed to load settings: ...")`, a `ClaudeSDKError`. With a sandbox set,
/// `_build_settings_value` raises it while `ClaudeSDKClient.connect` and `query()` build the command, after the CLI
/// is found and before it starts, when the settings it merges the sandbox into cannot be read, decoded or parsed,
/// or hold no object. `Display` is the message of the Python exception, and [`cause`](Self::cause), which is also
/// its `source`, the `__cause__`.
#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
#[error("Failed to load settings: {cause}")]
pub struct SettingsLoadError {
    #[source]
    cause: SettingsLoadCauseError,
}

impl SettingsLoadError {
    pub(crate) fn new(cause: SettingsLoadCauseError) -> Self {
        Self { cause }
    }

    /// The exception Python caught, whose `str()` follows `Failed to load settings: ` in the message.
    pub fn cause(&self) -> &SettingsLoadCauseError {
        &self.cause
    }
}
