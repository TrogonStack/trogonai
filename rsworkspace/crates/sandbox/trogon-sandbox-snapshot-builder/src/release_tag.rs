use trogon_sandbox_contract::{LeaseReleased, RetentionPolicy, SnapshotReceipt};

/// The tag a release asks to move and the snapshot it moves to.
pub(crate) struct ReleaseTag<'a> {
    pub(crate) event: &'a LeaseReleased,
    pub(crate) receipt: &'a SnapshotReceipt,
    pub(crate) tag_name: &'a str,
    pub(crate) retention: Option<RetentionPolicy>,
}

impl ReleaseTag<'_> {
    pub(crate) fn correlation_id(&self) -> &str {
        self.event.correlation_id.as_ref().map_or("unknown", |id| id.as_str())
    }
}
