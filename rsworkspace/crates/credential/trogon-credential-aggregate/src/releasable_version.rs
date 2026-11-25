use trogon_credential_domain::CredentialRef;
use trogonai_proto::gateway::credentials::{CredentialStateSnapshotCase, state_v1};

use crate::CredentialProtoDecodeError;
use crate::proto::{active_credential_ref, decode_message_field};

/// The version a credential may release in the state its stream folded to.
///
/// Only an active credential releases, and a pending rotation still releases
/// the version it would replace. Every other state releases nothing: a write
/// that has not landed, a failed write, and a revoked, destroying, destroyed,
/// or cleanup-failed credential. A state that does not decode is an error, never
/// a release.
pub fn releasable_version(
    state: &state_v1::CredentialStateSnapshot,
) -> Result<Option<CredentialRef>, CredentialProtoDecodeError> {
    match state.state.as_ref() {
        Some(CredentialStateSnapshotCase::Active(active)) => active_credential_ref(active).map(Some),
        Some(CredentialStateSnapshotCase::RotationPending(rotation)) => {
            let active = decode_message_field("rotation_pending.active", &rotation.active)?;
            active_credential_ref(active).map(Some)
        }
        None
        | Some(
            CredentialStateSnapshotCase::Missing(_)
            | CredentialStateSnapshotCase::PendingWrite(_)
            | CredentialStateSnapshotCase::WriteFailed(_)
            | CredentialStateSnapshotCase::Revoked(_)
            | CredentialStateSnapshotCase::DestroyRequested(_)
            | CredentialStateSnapshotCase::Destroyed(_)
            | CredentialStateSnapshotCase::CleanupFailed(_),
        ) => Ok(None),
    }
}

#[cfg(test)]
mod tests;
