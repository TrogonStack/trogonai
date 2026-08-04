#[cfg(test)]
#[path = "command_tests.rs"]
mod command_tests;

use crate::CommandTrigger;
use crate::CommandTriggerInput;
pub use crate::command_trigger::CommandTriggerError;
use serde::{Deserialize, Serialize};

/// A bridge-level instruction recognized in message text. Commands are
/// consumed by the bridge and never forwarded to the agent, so the chat
/// surface owns its own control vocabulary regardless of what the agent
/// behind it happens to understand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    /// Release the conversation's current session. The agent binding is
    /// untouched; only the ephemeral session is replaced.
    NewSession,
}

/// The trigger vocabulary a bridge recognizes, matched against the whole first
/// token of a message. Configurable because the leading marker is a channel
/// affordance rather than a domain concept.
#[derive(Debug, Clone)]
pub struct CommandTriggers {
    new_session: Vec<CommandTrigger>,
}

impl Default for CommandTriggers {
    #[allow(clippy::expect_used)]
    fn default() -> Self {
        Self::new(["/new", "/reset"]).expect("the default triggers are single non-empty tokens")
    }
}

/// Message text after command extraction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedText {
    pub command: Option<Command>,
    /// What remains once the trigger is removed, or the message unchanged when
    /// no trigger matched. `None` when nothing but the command was sent.
    pub body: Option<String>,
}

impl CommandTriggers {
    pub fn new<I, T>(new_session: I) -> Result<Self, CommandTriggerError>
    where
        I: IntoIterator<Item = T>,
        T: Into<CommandTriggerInput>,
    {
        let new_session = new_session
            .into_iter()
            .map(|trigger| CommandTrigger::try_from(trigger.into()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { new_session })
    }

    /// Split a message into its command (if the first token is a trigger) and
    /// the remaining text, which becomes the first prompt of whatever the
    /// command sets up.
    ///
    /// `recipient_account` is this bridge's account on the channel. Channels
    /// let a user address a command to one bot among several by suffixing it
    /// (`/new@somebot`); only an absent suffix or a suffix that matches this
    /// account is recognized.
    pub fn parse(&self, text: &str, recipient_account: &str) -> ParsedText {
        let trimmed = text.trim_start();
        let (head, rest) = match trimmed.find(char::is_whitespace) {
            Some(index) => (&trimmed[..index], trimmed[index..].trim()),
            None => (trimmed, ""),
        };

        let (token, addressed_to) = match head.split_once('@') {
            Some((token, account)) => (token, Some(account)),
            None => (head, None),
        };
        let token = token.to_ascii_lowercase();
        let addressed_to_us = match addressed_to {
            None => true,
            Some(account) => account.eq_ignore_ascii_case(recipient_account),
        };
        let command =
            (addressed_to_us && self.new_session.iter().any(|t| t.as_str() == token)).then_some(Command::NewSession);

        let body = match command {
            Some(_) => rest,
            None => text,
        };
        ParsedText {
            command,
            body: (!body.trim().is_empty()).then(|| body.to_string()),
        }
    }
}
