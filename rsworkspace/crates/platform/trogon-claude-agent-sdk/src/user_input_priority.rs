use serde::{Deserialize, Serialize};

/// `SDKUserMessage.priority`: when the CLI takes a queued user message.
///
/// Both pinned upstream SDKs write the value without validating it, so a spelling this crate does not know yet is
/// kept verbatim in [`UserInputPriority::Unknown`].
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UserInputPriority {
    Now,
    Next,
    Later,
    #[serde(untagged)]
    Unknown(String),
}

impl UserInputPriority {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Now => "now",
            Self::Next => "next",
            Self::Later => "later",
            Self::Unknown(priority) => priority,
        }
    }
}

impl From<String> for UserInputPriority {
    fn from(priority: String) -> Self {
        match priority.as_str() {
            "now" => Self::Now,
            "next" => Self::Next,
            "later" => Self::Later,
            _ => Self::Unknown(priority),
        }
    }
}
