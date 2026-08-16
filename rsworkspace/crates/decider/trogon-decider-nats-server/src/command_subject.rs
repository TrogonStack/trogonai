//! The ADR#0057 projection between a command type and the subject that carries it.
//!
//! Both directions live in one type on purpose. They are inverses, and an
//! inverse pair split across two modules is a pair that can drift: a host that
//! subscribes under one rule and resolves under another silently stops routing
//! the commands whose names disagree.

use trogon_decider_wasm_runtime::{CommandType, CommandTypeError};
use trogon_nats::{DottedNatsToken, SubjectTokenViolationError, SubjectViolationError, validate_published_subject};

use crate::constants::TYPE_URL_PREFIX;

/// The configured subject namespace a host answers under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubjectPrefix(DottedNatsToken);

impl SubjectPrefix {
    /// Creates a prefix after rejecting anything NATS cannot carry as a token.
    pub fn new(value: impl AsRef<str>) -> Result<Self, SubjectTokenViolationError> {
        DottedNatsToken::new(value).map(Self)
    }

    /// Returns the prefix as written.
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
}

impl std::fmt::Display for SubjectPrefix {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.as_str().fmt(f)
    }
}

/// The subject/command-type projection for one configured prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSubjects {
    prefix: SubjectPrefix,
}

impl CommandSubjects {
    /// Binds the projection to a prefix.
    pub const fn new(prefix: SubjectPrefix) -> Self {
        Self { prefix }
    }

    /// Returns the prefix this projection is bound to.
    pub const fn prefix(&self) -> &SubjectPrefix {
        &self.prefix
    }

    /// The single subscription that covers every command type.
    ///
    /// One subtree rather than one subscription per command type, because the
    /// routable set changes under `DeciderRegistryHandle::activate` and a
    /// per-type subscription set would have to be resynchronized on every
    /// rollout.
    pub fn subscription_pattern(&self) -> String {
        format!("{}.>", self.prefix)
    }

    /// Projects a command type onto the subject that carries it.
    ///
    /// Fails rather than truncating when the result would break ADR#0055's
    /// limits: a subject the host cannot legally publish is a command it
    /// cannot legally receive, and discovering that at registration is far
    /// cheaper than discovering it on the first request.
    pub fn subject_for(&self, command_type: &CommandType) -> Result<String, CommandSubjectError> {
        let full_name =
            command_type
                .as_str()
                .strip_prefix(TYPE_URL_PREFIX)
                .ok_or_else(|| CommandSubjectError::NotATypeUrl {
                    command_type: command_type.clone(),
                })?;
        if full_name.is_empty() {
            return Err(CommandSubjectError::NotATypeUrl {
                command_type: command_type.clone(),
            });
        }

        let subject = format!("{}.{full_name}", self.prefix);
        validate_published_subject(&subject).map_err(|source| CommandSubjectError::Subject {
            subject: subject.clone(),
            source,
        })?;
        Ok(subject)
    }

    /// Recovers the command type a subject names.
    pub fn command_type_for(&self, subject: &str) -> Result<CommandType, CommandSubjectError> {
        let full_name = subject
            .strip_prefix(self.prefix.as_str())
            .and_then(|rest| rest.strip_prefix('.'))
            .ok_or_else(|| CommandSubjectError::PrefixMismatch {
                subject: subject.to_owned(),
                prefix: self.prefix.as_str().to_owned(),
            })?;
        if full_name.is_empty() {
            return Err(CommandSubjectError::EmptyCommandName {
                subject: subject.to_owned(),
            });
        }

        CommandType::new(format!("{TYPE_URL_PREFIX}{full_name}")).map_err(|source| CommandSubjectError::CommandType {
            subject: subject.to_owned(),
            source,
        })
    }
}

/// Why a subject and a command type could not be projected onto each other.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CommandSubjectError {
    /// The command type is not a `type.googleapis.com/` URL, so it has no
    /// protobuf full name to place in a subject.
    #[error("command type '{command_type}' is not a '{TYPE_URL_PREFIX}' type url")]
    NotATypeUrl { command_type: CommandType },
    /// The projected subject breaks ADR#0055's shape or limits.
    #[error("command subject '{subject}' is not conformant: {source}")]
    Subject {
        subject: String,
        #[source]
        source: SubjectViolationError,
    },
    /// The subject was not published under this host's prefix.
    #[error("subject '{subject}' does not start with the configured prefix '{prefix}'")]
    PrefixMismatch { subject: String, prefix: String },
    /// The subject was the bare prefix, naming no command at all.
    #[error("subject '{subject}' names no command after its prefix")]
    EmptyCommandName { subject: String },
    /// The recovered name is not a usable command type.
    #[error("subject '{subject}' does not name a valid command type: {source}")]
    CommandType {
        subject: String,
        #[source]
        source: CommandTypeError,
    },
}

#[cfg(test)]
mod tests;
