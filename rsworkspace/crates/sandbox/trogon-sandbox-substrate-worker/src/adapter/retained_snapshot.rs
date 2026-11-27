use std::collections::HashSet;

use super::SubstrateError;
use crate::proto;

pub(super) struct SnapshotEvidence {
    pub generation: i32,
    pub uuid: String,
}

fn refused(reason: &str) -> SubstrateError {
    SubstrateError::Ownership(format!("retained snapshot is unconfirmed: {reason}"))
}

pub(super) fn durable_memory_snapshot(actor: &proto::Actor) -> Result<Option<SnapshotEvidence>, SubstrateError> {
    let status = actor
        .status
        .as_ref()
        .ok_or_else(|| refused("actor status is missing"))?;
    if status.state != proto::ActorState::Suspended as i32 {
        return Err(refused("actor is no longer suspended"));
    }
    let latest = status.last_assigned_generation;
    if latest <= 0 {
        return Err(refused("latest suspend generation is missing"));
    }
    let mut generations = HashSet::new();
    for snapshot in &status.snapshots {
        if snapshot.generation <= 0 || snapshot.generation > latest || !generations.insert(snapshot.generation) {
            return Err(refused("snapshot generations are inconsistent"));
        }
        if !matches!(
            proto::SnapshotOwner::try_from(snapshot.owner),
            Ok(proto::SnapshotOwner::Actor | proto::SnapshotOwner::Tag)
        ) || !matches!(
            proto::SnapshotFidelity::try_from(snapshot.fidelity),
            Ok(proto::SnapshotFidelity::Volumes | proto::SnapshotFidelity::Rootfs | proto::SnapshotFidelity::Memory)
        ) || snapshot.durable_snapshot.as_ref().is_some_and(|storage| {
            !matches!(
                proto::SnapshotStorageState::try_from(storage.state),
                Ok(proto::SnapshotStorageState::Pending | proto::SnapshotStorageState::Completed)
            )
        }) {
            return Err(refused("snapshot has an unsupported owner, fidelity or storage state"));
        }
    }
    let snapshot = status
        .snapshots
        .iter()
        .find(|snapshot| snapshot.generation == latest)
        .ok_or_else(|| refused("latest snapshot generation is absent"))?;
    if snapshot.uuid.is_empty() || snapshot.owner != proto::SnapshotOwner::Actor as i32 {
        return Err(refused("latest snapshot is not an identified actor capture"));
    }
    if snapshot.fidelity != proto::SnapshotFidelity::Memory as i32 {
        return Err(refused("latest snapshot did not preserve memory"));
    }
    let Some(storage) = snapshot.durable_snapshot.as_ref() else {
        return Ok(None);
    };
    if storage.state == proto::SnapshotStorageState::Pending as i32 {
        return Ok(None);
    }
    let object = storage
        .object
        .as_ref()
        .ok_or_else(|| refused("completed storage has no snapshot object"))?;
    let uri = object
        .snapshot_uri
        .parse::<tonic::codegen::http::Uri>()
        .map_err(|_| refused("snapshot object URI is malformed"))?;
    if uri.scheme().is_none() || uri.authority().is_none() {
        return Err(refused("snapshot object URI must be absolute"));
    }
    Ok(Some(SnapshotEvidence {
        generation: snapshot.generation,
        uuid: snapshot.uuid.clone(),
    }))
}
