//! The subject an audit record is published on (ADR#0086 Decision 2).
//!
//! ```text
//! platform.v1.audit.decision.{project}.{allow|deny|would_deny}
//! platform.v1.audit.credential.{project}.{release|inject}
//! platform.v1.audit.change.{project}.{policy|credential}
//! platform.v1.audit.integrity.{component}.{start|chain_reset}
//! platform.v1.audit.checkpoint.{component}.signed
//! ```
//!
//! The prefix is the fixed token `platform` and no tenant token appears: the
//! tenant leads inside the `PLATFORM` account. Per-request values are payload
//! fields, so a subject never carries an agent, a tool, a host or a method.

use trogon_nats::NatsToken;

use crate::change_terminal::ChangeTerminal;
use crate::checkpoint_terminal::CheckpointTerminal;
use crate::constants::{
    SUBJECT_BINDING_VERSION, SUBJECT_KIND_CHANGE, SUBJECT_KIND_CHECKPOINT, SUBJECT_KIND_CREDENTIAL,
    SUBJECT_KIND_DECISION, SUBJECT_KIND_INTEGRITY, SUBJECT_PREFIX, SUBJECT_ROOT,
};
use crate::credential_terminal::CredentialTerminal;
use crate::decision_terminal::DecisionTerminal;
use crate::integrity_terminal::IntegrityTerminal;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum AuditSubject {
    Decision {
        project: NatsToken,
        terminal: DecisionTerminal,
    },
    Credential {
        project: NatsToken,
        terminal: CredentialTerminal,
    },
    Change {
        project: NatsToken,
        terminal: ChangeTerminal,
    },
    Integrity {
        component: NatsToken,
        terminal: IntegrityTerminal,
    },
    Checkpoint {
        component: NatsToken,
        terminal: CheckpointTerminal,
    },
}

impl AuditSubject {
    pub fn decision(project: NatsToken, terminal: DecisionTerminal) -> Self {
        Self::Decision { project, terminal }
    }

    pub fn credential(project: NatsToken, terminal: CredentialTerminal) -> Self {
        Self::Credential { project, terminal }
    }

    pub fn change(project: NatsToken, terminal: ChangeTerminal) -> Self {
        Self::Change { project, terminal }
    }

    pub fn integrity(component: NatsToken, terminal: IntegrityTerminal) -> Self {
        Self::Integrity { component, terminal }
    }

    pub fn checkpoint(component: NatsToken, terminal: CheckpointTerminal) -> Self {
        Self::Checkpoint { component, terminal }
    }

    /// The concrete subject.
    ///
    /// Rendering cannot fail: every dynamic position is a [`NatsToken`], which
    /// has no dot, wildcard or whitespace and is at most 128 characters, and every
    /// other token is a fixed literal. The tests hold the result to the workspace's
    /// published-subject rules at the longest token each position allows.
    ///
    /// The request-id-shaped token check is not applied: a project id is a durable
    /// resource identity whose format this crate does not own, and it may be
    /// UUID-shaped.
    pub fn render(&self) -> String {
        let (kind, scope, terminal) = match self {
            Self::Decision { project, terminal } => (SUBJECT_KIND_DECISION, project, terminal.as_str()),
            Self::Credential { project, terminal } => (SUBJECT_KIND_CREDENTIAL, project, terminal.as_str()),
            Self::Change { project, terminal } => (SUBJECT_KIND_CHANGE, project, terminal.as_str()),
            Self::Integrity { component, terminal } => (SUBJECT_KIND_INTEGRITY, component, terminal.as_str()),
            Self::Checkpoint { component, terminal } => (SUBJECT_KIND_CHECKPOINT, component, terminal.as_str()),
        };
        format!("{SUBJECT_PREFIX}.{SUBJECT_BINDING_VERSION}.{SUBJECT_ROOT}.{kind}.{scope}.{terminal}")
    }
}

#[cfg(test)]
mod tests;
