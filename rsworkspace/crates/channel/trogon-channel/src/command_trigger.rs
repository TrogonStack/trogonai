//! A validated command trigger token.

use crate::CommandTriggerInput;

/// Why a [`CommandTriggerInput`] could not become a [`CommandTrigger`].
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CommandTriggerError {
    #[error("command trigger must not be empty")]
    Empty,
    #[error("command trigger must be a single token")]
    MultipleTokens,
}

/// One normalized trigger matched against the first token of a message.
/// Guarantees a non-empty, single-token, lowercased value at construction.
///
/// Lowercasing is Unicode-aware rather than ASCII-only, because a trigger is
/// chat text a person types and nothing restricts it to ASCII. Matching folds
/// the incoming token the same way (see [`crate::CommandTriggers::parse`]); an
/// ASCII-only fold would leave `/Nuevo` matching but `/AÑADIR` not, which is a
/// distinction no operator would predict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandTrigger(String);

impl CommandTrigger {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<CommandTriggerInput> for CommandTrigger {
    type Error = CommandTriggerError;

    fn try_from(input: CommandTriggerInput) -> Result<Self, Self::Error> {
        let trigger = input.as_str().trim().to_lowercase();
        if trigger.is_empty() {
            return Err(CommandTriggerError::Empty);
        }
        if trigger.split_whitespace().count() != 1 {
            return Err(CommandTriggerError::MultipleTokens);
        }
        Ok(Self(trigger))
    }
}
