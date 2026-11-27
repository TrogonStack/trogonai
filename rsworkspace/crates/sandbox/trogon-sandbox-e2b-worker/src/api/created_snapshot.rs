use crate::snapshots::SnapshotTemplateId;

/// The template a snapshot capture created, as the cleanup identity.
#[derive(Debug)]
pub(crate) struct CreatedSnapshot {
    pub(crate) template_id: SnapshotTemplateId,
}
