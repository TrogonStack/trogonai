use std::collections::BTreeMap;
use std::time::SystemTime;

use crate::delegation_permission::DelegationPermission;
use crate::egress_permission::EgressPermission;
use crate::hold::Hold;
use crate::model_ceiling::ModelCeiling;
use crate::rule_kind::RuleKind;
use crate::rule_ref::RuleRef;
use crate::stream_revision::StreamRevision;
use crate::subject::Subject;

/// One policy history as the projection last saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct History<T> {
    revision: StreamRevision,
    value: T,
}

/// An immutable, revision-stamped copy of the bindings an enforcement point
/// evaluates against (ADR#0071 Decision 7).
///
/// The enforcement point builds it from the policy streams and does not read a
/// stream per request. Every history carries the revision the projection
/// reflected, so the evaluator can say exactly what it consulted, and the
/// snapshot carries `as_of`, the last time the projector vouched for it.
///
/// `as_of` advances on a heartbeat from the projector, not only when a binding
/// changes: a binding nobody has touched for a month has an old entry and a
/// perfectly current projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicySnapshot {
    as_of: SystemTime,
    holds: BTreeMap<Subject, History<Vec<Hold>>>,
    model_ceilings: BTreeMap<Subject, History<ModelCeiling>>,
    egress_permissions: BTreeMap<Subject, History<EgressPermission>>,
    delegation_permissions: BTreeMap<Subject, History<DelegationPermission>>,
}

impl PolicySnapshot {
    /// Starts a snapshot that the producer last vouched for at `as_of`.
    pub fn builder(as_of: SystemTime) -> PolicySnapshotBuilder {
        PolicySnapshotBuilder {
            snapshot: Self {
                as_of,
                holds: BTreeMap::new(),
                model_ceilings: BTreeMap::new(),
                egress_permissions: BTreeMap::new(),
                delegation_permissions: BTreeMap::new(),
            },
        }
    }

    /// When the producer last vouched for this snapshot.
    pub fn as_of(&self) -> SystemTime {
        self.as_of
    }

    /// The revision of one history, zero when the history does not exist.
    pub fn revision(&self, kind: RuleKind, subject: &Subject) -> StreamRevision {
        let revision = match kind {
            RuleKind::Hold => self.holds.get(subject).map(|history| history.revision),
            RuleKind::ModelCeiling => self.model_ceilings.get(subject).map(|history| history.revision),
            RuleKind::EgressPermission => self.egress_permissions.get(subject).map(|history| history.revision),
            RuleKind::DelegationPermission => self.delegation_permissions.get(subject).map(|history| history.revision),
        };
        revision.unwrap_or_else(StreamRevision::missing)
    }

    /// A reference to one history at the revision this snapshot holds.
    pub fn rule_ref(&self, kind: RuleKind, subject: &Subject) -> RuleRef {
        RuleRef::new(kind, subject.clone(), self.revision(kind, subject))
    }

    pub(crate) fn holds(&self, subject: &Subject) -> &[Hold] {
        self.holds.get(subject).map_or(&[], |history| history.value.as_slice())
    }

    pub(crate) fn model_ceiling(&self, subject: &Subject) -> Option<&ModelCeiling> {
        self.model_ceilings.get(subject).map(|history| &history.value)
    }

    pub(crate) fn egress_permission(&self, subject: &Subject) -> Option<&EgressPermission> {
        self.egress_permissions.get(subject).map(|history| &history.value)
    }

    pub(crate) fn delegation_permission(&self, subject: &Subject) -> Option<&DelegationPermission> {
        self.delegation_permissions.get(subject).map(|history| &history.value)
    }
}

/// Builds a [`PolicySnapshot`]. A later call for the same kind and subject
/// replaces the earlier one.
#[derive(Debug, Clone)]
pub struct PolicySnapshotBuilder {
    snapshot: PolicySnapshot,
}

impl PolicySnapshotBuilder {
    /// Records the holds currently placed on a subject.
    pub fn holds(
        mut self,
        subject: impl Into<Subject>,
        revision: impl Into<StreamRevision>,
        holds: impl IntoIterator<Item = Hold>,
    ) -> Self {
        self.snapshot.holds.insert(
            subject.into(),
            History {
                revision: revision.into(),
                value: holds.into_iter().collect(),
            },
        );
        self
    }

    /// Records the model ceiling bound to a subject.
    pub fn model_ceiling(
        mut self,
        subject: impl Into<Subject>,
        revision: impl Into<StreamRevision>,
        ceiling: ModelCeiling,
    ) -> Self {
        self.snapshot.model_ceilings.insert(
            subject.into(),
            History {
                revision: revision.into(),
                value: ceiling,
            },
        );
        self
    }

    /// Records the egress permission bound to a subject.
    pub fn egress_permission(
        mut self,
        subject: impl Into<Subject>,
        revision: impl Into<StreamRevision>,
        permission: EgressPermission,
    ) -> Self {
        self.snapshot.egress_permissions.insert(
            subject.into(),
            History {
                revision: revision.into(),
                value: permission,
            },
        );
        self
    }

    /// Records the delegation permission bound to a subject.
    pub fn delegation_permission(
        mut self,
        subject: impl Into<Subject>,
        revision: impl Into<StreamRevision>,
        permission: DelegationPermission,
    ) -> Self {
        self.snapshot.delegation_permissions.insert(
            subject.into(),
            History {
                revision: revision.into(),
                value: permission,
            },
        );
        self
    }

    /// Finishes the snapshot.
    pub fn build(self) -> PolicySnapshot {
        self.snapshot
    }
}
