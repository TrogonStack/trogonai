//! Mission lifecycle per draft section "Mission": creation, approval, log
//! accumulation, and completion/termination. Wire bodies
//! ([`trogon_identity_types::aauth::mission::MissionProposal`],
//! `MissionBlob`, `MissionLogEntry`, `MissionCompletion`) are reused directly
//! from `trogon-identity-types`; this module adds the server-side state
//! machine around them.

use sha2::{Digest, Sha256};
use trogon_identity_types::aauth::MissionRef;
use trogon_identity_types::aauth::mission::{MissionBlob, MissionLogEntry, MissionStatus};

/// Opaque mission identifier: the base64url-encoded `s256` hash of the
/// approved [`MissionBlob`] bytes, per "Mission Approval" (the same value
/// carried as `mission.s256` in [`MissionRef`]).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MissionId(pub String);

impl MissionId {
    #[must_use]
    pub fn from_blob_bytes(blob_bytes: &[u8]) -> Self {
        let digest = Sha256::digest(blob_bytes);
        Self(base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            digest,
        ))
    }
}

/// Server-side mission record: the byte-exact approved blob (preserved
/// verbatim so `s256` can be recomputed/verified later, per `MissionBlob`'s
/// own doc comment), the accumulated log, and lifecycle status.
#[derive(Clone, Debug)]
pub struct Mission {
    pub id: MissionId,
    /// The exact bytes returned to the agent at approval time. MUST NOT be
    /// reconstructed by re-serializing [`MissionBlob`] -- see "Mission
    /// Approval": hashing must operate over the bytes actually served.
    pub blob_bytes: Vec<u8>,
    pub blob: MissionBlob,
    pub status: MissionStatus,
    pub log: Vec<MissionLogEntry>,
}

impl Mission {
    #[must_use]
    pub fn approve(blob_bytes: Vec<u8>, blob: MissionBlob) -> Self {
        let id = MissionId::from_blob_bytes(&blob_bytes);
        Self {
            id,
            blob_bytes,
            blob,
            status: MissionStatus::Active,
            log: Vec::new(),
        }
    }

    #[must_use]
    pub fn mission_ref(&self) -> MissionRef {
        MissionRef {
            approver: self.blob.approver.clone(),
            s256: self.id.0.clone(),
        }
    }

    /// Appends one governed event to the mission log, per "Mission Log":
    /// every permission/audit/interaction/clarification event tied to an
    /// active mission is recorded.
    pub fn append_log(&mut self, entry: MissionLogEntry) {
        self.log.push(entry);
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.status == MissionStatus::Active
    }

    /// Terminates the mission per "Mission Completion" / "Mission
    /// Management": once terminated, a mission cannot transition back to
    /// active ("two states only").
    pub fn complete(&mut self) {
        self.status = MissionStatus::Terminated;
    }
}

/// Mission context visible to policy decisions, per "PS Response": mission
/// scope can influence what the PS grants for a given resource request.
#[derive(Clone, Debug)]
pub struct MissionContext {
    pub mission_ref: MissionRef,
    pub blob: MissionBlob,
    pub status: MissionStatus,
}

impl From<&Mission> for MissionContext {
    fn from(mission: &Mission) -> Self {
        Self {
            mission_ref: mission.mission_ref(),
            blob: mission.blob.clone(),
            status: mission.status.clone(),
        }
    }
}

#[cfg(test)]
mod tests;
