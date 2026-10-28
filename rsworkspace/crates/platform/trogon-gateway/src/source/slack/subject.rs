use trogon_nats::{NatsToken, SubjectTokenViolationError};

use super::system_notice::SystemNoticeType;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlackSubject(String);

fn token(raw: &str) -> Result<NatsToken, SubjectTokenViolationError> {
    if let Some(control) = raw.chars().find(char::is_ascii_control) {
        return Err(SubjectTokenViolationError::InvalidCharacter(control));
    }
    NatsToken::new(raw)
}

impl SlackSubject {
    pub fn event(prefix: &NatsToken, event_type: &str) -> Result<Self, SubjectTokenViolationError> {
        Ok(Self(format!("{prefix}.event.{}", token(event_type)?)))
    }

    pub fn interaction(prefix: &NatsToken, interaction_type: &str) -> Result<Self, SubjectTokenViolationError> {
        Ok(Self(format!("{prefix}.interaction.{}", token(interaction_type)?)))
    }

    pub fn command(prefix: &NatsToken, command: &str) -> Result<Self, SubjectTokenViolationError> {
        let name = command.trim_start_matches('/');
        Ok(Self(format!("{prefix}.command.{}", token(name)?)))
    }

    pub fn system(prefix: &NatsToken, notice: SystemNoticeType) -> Self {
        Self(format!("{prefix}.system.{}", notice.as_str()))
    }

    pub fn unroutable(prefix: &NatsToken) -> Self {
        Self(format!("{prefix}.unroutable"))
    }

    pub fn stream_filters(prefix: &NatsToken) -> Vec<String> {
        ["event.>", "interaction.>", "command.>", "system.>", "unroutable"]
            .into_iter()
            .map(|filter| format!("{prefix}.{filter}"))
            .collect()
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_string(self) -> String {
        self.0
    }
}

#[cfg(test)]
mod tests;
