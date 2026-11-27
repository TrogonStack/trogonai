/// The queue group every Hyperlight worker joins, so one worker takes each dispatch.
pub(crate) const QUEUE_GROUP: &str = "sandbox-provider-hyperlight";

/// Media type of the single layer of a Hyperlight snapshot artifact.
pub const HYPERLIGHT_SNAPSHOT_MEDIA_TYPE: &str = "application/vnd.trogon.hyperlight.snapshot.v1";
