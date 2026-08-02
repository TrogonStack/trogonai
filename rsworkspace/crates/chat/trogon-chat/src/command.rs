use serde::{Deserialize, Serialize};

/// A bridge-level instruction recognized in message text. Commands are
/// consumed by the bridge and never forwarded to the agent, so the chat
/// surface owns its own control vocabulary regardless of what the agent
/// behind it happens to understand.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatCommand {
    /// Release the conversation's current session. The agent binding is
    /// untouched; only the ephemeral session is replaced.
    NewSession,
}

#[derive(Debug, thiserror::Error)]
pub enum CommandTriggerError {
    #[error("command trigger must not be empty")]
    Empty,
    #[error("command trigger {0:?} must be a single token")]
    NotASingleToken(String),
}

/// The trigger vocabulary a bridge recognizes, matched against the whole first
/// token of a message. Configurable because the leading marker is a channel
/// affordance rather than a domain concept.
#[derive(Debug, Clone)]
pub struct CommandTriggers {
    new_session: Vec<String>,
}

impl Default for CommandTriggers {
    fn default() -> Self {
        Self {
            new_session: vec!["/new".to_string(), "/reset".to_string()],
        }
    }
}

/// Message text after command extraction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedText {
    pub command: Option<ChatCommand>,
    /// What remains once the trigger is removed, or the message unchanged when
    /// no trigger matched. `None` when nothing but the command was sent.
    pub body: Option<String>,
}

impl CommandTriggers {
    pub fn new(new_session: impl IntoIterator<Item = String>) -> Result<Self, CommandTriggerError> {
        let new_session = new_session
            .into_iter()
            .map(|trigger| {
                let trigger = trigger.trim().to_ascii_lowercase();
                if trigger.is_empty() {
                    return Err(CommandTriggerError::Empty);
                }
                if trigger.split_whitespace().count() != 1 {
                    return Err(CommandTriggerError::NotASingleToken(trigger));
                }
                Ok(trigger)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { new_session })
    }

    /// Split a message into its command (if the first token is a trigger) and
    /// the remaining text, which becomes the first prompt of whatever the
    /// command sets up.
    pub fn parse(&self, text: &str) -> ParsedText {
        let trimmed = text.trim_start();
        let (head, rest) = match trimmed.find(char::is_whitespace) {
            Some(index) => (&trimmed[..index], trimmed[index..].trim()),
            None => (trimmed, ""),
        };

        // Channels let a user address a command to one bot account among
        // several by suffixing it (`/new@somebot`). The suffix selects the
        // recipient and is not part of the trigger.
        let token = head.split('@').next().unwrap_or(head).to_ascii_lowercase();
        let command = self.new_session.contains(&token).then_some(ChatCommand::NewSession);

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_trigger_yields_a_command_and_no_body() {
        let parsed = CommandTriggers::default().parse("/new");
        assert_eq!(parsed.command, Some(ChatCommand::NewSession));
        assert_eq!(parsed.body, None);
    }

    #[test]
    fn trailing_text_becomes_the_body() {
        let parsed = CommandTriggers::default().parse("/reset  ship the thing ");
        assert_eq!(parsed.command, Some(ChatCommand::NewSession));
        assert_eq!(parsed.body.as_deref(), Some("ship the thing"));
    }

    #[test]
    fn account_suffix_and_case_do_not_defeat_the_trigger() {
        let parsed = CommandTriggers::default().parse("/New@SomeBot hello");
        assert_eq!(parsed.command, Some(ChatCommand::NewSession));
        assert_eq!(parsed.body.as_deref(), Some("hello"));
    }

    #[test]
    fn a_trigger_that_is_only_a_prefix_of_the_token_is_not_a_command() {
        let parsed = CommandTriggers::default().parse("/newsletter please");
        assert_eq!(parsed.command, None);
        assert_eq!(parsed.body.as_deref(), Some("/newsletter please"));
    }

    #[test]
    fn ordinary_text_passes_through_unchanged() {
        let parsed = CommandTriggers::default().parse("  keep   my spacing  ");
        assert_eq!(parsed.command, None);
        assert_eq!(parsed.body.as_deref(), Some("  keep   my spacing  "));
    }

    #[test]
    fn a_trigger_in_the_middle_is_not_a_command() {
        let parsed = CommandTriggers::default().parse("say /new out loud");
        assert_eq!(parsed.command, None);
        assert_eq!(parsed.body.as_deref(), Some("say /new out loud"));
    }

    #[test]
    fn triggers_are_configurable() {
        let triggers = CommandTriggers::new(["!Rotate".to_string()]).expect("valid triggers");
        assert_eq!(triggers.parse("!rotate").command, Some(ChatCommand::NewSession));
        assert_eq!(triggers.parse("/new").command, None);
    }

    #[test]
    fn blank_and_multi_token_triggers_are_rejected() {
        assert!(matches!(
            CommandTriggers::new(["  ".to_string()]),
            Err(CommandTriggerError::Empty)
        ));
        assert!(matches!(
            CommandTriggers::new(["/new session".to_string()]),
            Err(CommandTriggerError::NotASingleToken(_))
        ));
    }
}
