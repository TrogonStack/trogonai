use std::collections::HashSet;

use trogon_sandbox_contract::{EnvDigest, LegacySnapshotPolicy};

use crate::{EnvironmentError, RestoreVerdict};

/// A worker's own environment digest, the digests its operator approved, and the policy for unlabeled snapshots.
///
/// Installation never authorizes selection: a digest outside the approved set is not runnable even when its binaries
/// are installed.
#[derive(Debug, Clone, Default, Eq, PartialEq)]
pub struct EnvironmentPolicy {
    /// `None` for a worker that does not compute a digest: none is written or checked.
    pub own: Option<EnvDigest>,
    /// Digests this node may run. Empty approves `own` alone.
    pub approved: HashSet<EnvDigest>,
    /// What to do with a snapshot that records no digest.
    pub legacy_snapshots: LegacySnapshotPolicy,
}

impl EnvironmentPolicy {
    /// A policy for a worker whose environment digest is `own`, approving that digest alone.
    pub fn new(own: EnvDigest) -> Self {
        Self {
            own: Some(own),
            ..Self::default()
        }
    }

    /// Replaces the approved set.
    pub fn with_approved(mut self, approved: impl IntoIterator<Item = EnvDigest>) -> Self {
        self.approved = approved.into_iter().collect();
        self
    }

    /// Replaces the policy for unlabeled snapshots.
    pub fn with_legacy_snapshots(mut self, policy: LegacySnapshotPolicy) -> Self {
        self.legacy_snapshots = policy;
        self
    }

    /// Whether this node may run `digest`.
    pub fn approves(&self, digest: &EnvDigest) -> bool {
        if self.approved.is_empty() {
            return self.own.as_ref() == Some(digest);
        }
        self.approved.contains(digest)
    }

    /// Run once at start: a worker whose own digest is not approved must not serve leases.
    pub fn check_startup(&self) -> Result<(), EnvironmentError> {
        match &self.own {
            Some(own) if !self.approves(own) => Err(EnvironmentError::NotApproved { own: own.clone() }),
            _ => Ok(()),
        }
    }

    /// Decides whether a snapshot labeled `snapshot` may be restored here.
    pub fn check_restore(&self, snapshot: Option<&EnvDigest>) -> Result<RestoreVerdict, EnvironmentError> {
        let Some(own) = &self.own else {
            return Ok(RestoreVerdict::Unchecked);
        };
        match snapshot {
            Some(snapshot) if snapshot == own && self.approves(snapshot) => Ok(RestoreVerdict::Matched),
            Some(snapshot) if snapshot == own => Err(EnvironmentError::NotApproved { own: own.clone() }),
            Some(snapshot) => Err(EnvironmentError::Mismatch {
                snapshot: snapshot.clone(),
                own: own.clone(),
            }),
            None => match self.legacy_snapshots {
                LegacySnapshotPolicy::AllowWithWarning => Ok(RestoreVerdict::LegacyUnlabeled),
                LegacySnapshotPolicy::Refuse => Err(EnvironmentError::LegacyRefused),
            },
        }
    }
}
