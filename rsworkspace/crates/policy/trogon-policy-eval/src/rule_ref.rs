use std::fmt;

use crate::rule_kind::RuleKind;
use crate::stream_revision::StreamRevision;
use crate::subject::Subject;

/// A reference to one policy history at the revision that was read: the kind,
/// the subject, and the stream revision (ADR#0071 Decision 7).
///
/// With the `proto` feature it converts from the `PolicyRuleRef` message.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RuleRef {
    kind: RuleKind,
    subject: Subject,
    revision: StreamRevision,
}

impl RuleRef {
    /// Names a history at a revision.
    pub fn new(kind: RuleKind, subject: Subject, revision: StreamRevision) -> Self {
        Self {
            kind,
            subject,
            revision,
        }
    }

    /// Which history.
    pub fn kind(&self) -> RuleKind {
        self.kind
    }

    /// Whose history.
    pub fn subject(&self) -> &Subject {
        &self.subject
    }

    /// The revision that was current, zero when the history did not exist.
    pub fn revision(&self) -> StreamRevision {
        self.revision
    }
}

impl fmt::Display for RuleRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} of {} at revision {}", self.kind, self.subject, self.revision)
    }
}
