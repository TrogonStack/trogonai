use serde::{Deserialize, Serialize};

/// Whether the worker replies when the command is accepted or when it completes.
///
/// It serializes as the JSON boolean `detached`. `Detached` makes the worker reply at once with
/// [`RunCommandResponse::Started`](crate::RunCommandResponse::Started); the caller then polls with
/// [`PollCommandRequest`](crate::PollCommandRequest).
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(from = "bool", into = "bool")]
pub enum CommandDetachment {
    #[default]
    Attached,
    Detached,
}

impl CommandDetachment {
    pub const fn is_detached(self) -> bool {
        matches!(self, Self::Detached)
    }
}

impl From<bool> for CommandDetachment {
    fn from(value: bool) -> Self {
        if value { Self::Detached } else { Self::Attached }
    }
}

impl From<CommandDetachment> for bool {
    fn from(value: CommandDetachment) -> Self {
        value.is_detached()
    }
}
