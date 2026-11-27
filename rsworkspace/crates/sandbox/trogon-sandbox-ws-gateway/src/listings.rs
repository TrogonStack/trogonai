use serde::Serialize;

use crate::lease_row::LeaseRow;
use crate::snapshot_row::SnapshotRow;
use crate::tag_row::TagRow;

#[derive(Serialize)]
pub(crate) struct LeaseListing<'a> {
    pub(crate) leases: Vec<LeaseRow<'a>>,
}

#[derive(Serialize)]
pub(crate) struct SnapshotListing<'a> {
    pub(crate) snapshots: Vec<SnapshotRow<'a>>,
}

#[derive(Serialize)]
pub(crate) struct TagListing<'a> {
    pub(crate) tags: Vec<TagRow<'a>>,
}
