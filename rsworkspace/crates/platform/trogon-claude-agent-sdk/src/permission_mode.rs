use serde::{Deserialize, Serialize};

/// Permission-mode values shared by the pinned Python and TypeScript SDKs.
///
/// Both upstream SDKs forward mode strings without validating them, so a mode
/// this crate does not know yet is kept verbatim in [`PermissionMode::Unknown`].
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionMode {
    Default,
    AcceptEdits,
    Plan,
    BypassPermissions,
    DontAsk,
    Auto,
    #[serde(untagged)]
    Unknown(String),
}

impl PermissionMode {
    pub(crate) fn as_str(&self) -> &str {
        match self {
            Self::Default => "default",
            Self::AcceptEdits => "acceptEdits",
            Self::Plan => "plan",
            Self::BypassPermissions => "bypassPermissions",
            Self::DontAsk => "dontAsk",
            Self::Auto => "auto",
            Self::Unknown(mode) => mode,
        }
    }
}

impl From<String> for PermissionMode {
    fn from(mode: String) -> Self {
        match mode.as_str() {
            "default" => Self::Default,
            "acceptEdits" => Self::AcceptEdits,
            "plan" => Self::Plan,
            "bypassPermissions" => Self::BypassPermissions,
            "dontAsk" => Self::DontAsk,
            "auto" => Self::Auto,
            _ => Self::Unknown(mode),
        }
    }
}
