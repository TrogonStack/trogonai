use serde::{Deserialize, Serialize};

/// What to do when an extract rule finds nothing to mask in the value.
///
/// `Error` refuses the sandbox, which keeps a credential from slipping through unmasked. `Warn` logs and leaves the
/// value as it is, visible to the sandbox. `Deny` withholds the value from the sandbox entirely. The default is
/// `Error`, and a binding omits it from the wire when it is the default.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExtractNoMatch {
    #[default]
    Error,
    Warn,
    Deny,
}

impl ExtractNoMatch {
    /// Reports whether this is the default policy.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}
