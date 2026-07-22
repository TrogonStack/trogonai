//! Mission lifecycle per draft section "Mission": creation, approval, log
//! accumulation, and completion/termination. Wire bodies
//! ([`trogon_identity_types::aauth::mission::MissionProposal`],
//! `MissionBlob`, `MissionLogEntry`, `MissionCompletion`) are reused directly
//! from `trogon-identity-types`; this module adds the server-side state
//! machine around them.

use sha2::{Digest, Sha256};
use trogon_identity_types::aauth::MissionRef;
use trogon_identity_types::aauth::mission::{MissionBlob, MissionLogEntry, MissionStatus, MissionTool};

/// Opaque mission identifier: the base64url-encoded `s256` hash of the
/// approved [`MissionBlob`] bytes, per "Mission Approval" (the same value
/// carried as `mission.s256` in [`MissionRef`]).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MissionId(String);

impl std::fmt::Display for MissionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl MissionId {
    #[must_use]
    pub fn from_blob_bytes(blob_bytes: &[u8]) -> Self {
        let digest = Sha256::digest(blob_bytes);
        Self(base64::Engine::encode(
            &base64::engine::general_purpose::URL_SAFE_NO_PAD,
            digest,
        ))
    }

    /// Wraps an s256 value read back from a verified claim or stored record
    /// for lookup purposes only -- never for constructing a new mission id,
    /// which must come from [`MissionId::from_blob_bytes`].
    #[must_use]
    pub fn from_s256(s256: impl Into<String>) -> Self {
        Self(s256.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Validated domain form of an approved mission, converted exactly once from
/// the [`MissionBlob`] wire body at the approval boundary so storage and
/// policy never carry the raw wire type.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovedMission {
    pub approver: String,
    pub agent: String,
    pub approved_at: String,
    pub description: String,
    pub approved_tools: Option<Vec<MissionTool>>,
    pub capabilities: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MissionValidationError {
    #[error("mission blob field {0} must not be empty")]
    EmptyField(&'static str),
}

impl TryFrom<MissionBlob> for ApprovedMission {
    type Error = MissionValidationError;

    fn try_from(blob: MissionBlob) -> Result<Self, Self::Error> {
        for (name, value) in [
            ("approver", &blob.approver),
            ("agent", &blob.agent),
            ("approved_at", &blob.approved_at),
            ("description", &blob.description),
        ] {
            if value.trim().is_empty() {
                return Err(MissionValidationError::EmptyField(name));
            }
        }
        Ok(Self {
            approver: blob.approver,
            agent: blob.agent,
            approved_at: blob.approved_at,
            description: blob.description,
            approved_tools: blob.approved_tools,
            capabilities: blob.capabilities,
        })
    }
}

/// Server-side mission record: the byte-exact approved blob (preserved
/// verbatim so `s256` can be recomputed/verified later, per `MissionBlob`'s
/// own doc comment), the accumulated log, and lifecycle status.
#[derive(Clone, Debug)]
pub struct Mission {
    pub id: MissionId,
    /// The exact bytes returned to the agent at approval time. MUST NOT be
    /// reconstructed by re-serializing the wire blob -- see "Mission
    /// Approval": hashing must operate over the bytes actually served.
    pub blob_bytes: Vec<u8>,
    pub approved: ApprovedMission,
    pub status: MissionStatus,
    pub log: Vec<MissionLogEntry>,
}

impl Mission {
    pub fn approve(blob_bytes: Vec<u8>, blob: MissionBlob) -> Result<Self, MissionValidationError> {
        let id = MissionId::from_blob_bytes(&blob_bytes);
        Ok(Self {
            id,
            blob_bytes,
            approved: ApprovedMission::try_from(blob)?,
            status: MissionStatus::Active,
            log: Vec::new(),
        })
    }

    #[must_use]
    pub fn mission_ref(&self) -> MissionRef {
        MissionRef {
            approver: self.approved.approver.clone(),
            s256: self.id.as_str().to_owned(),
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
    pub approved: ApprovedMission,
    pub status: MissionStatus,
}

impl From<&Mission> for MissionContext {
    fn from(mission: &Mission) -> Self {
        Self {
            mission_ref: mission.mission_ref(),
            approved: mission.approved.clone(),
            status: mission.status.clone(),
        }
    }
}

#[cfg(test)]
mod tests;
