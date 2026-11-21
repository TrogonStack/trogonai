use std::time::Duration;

pub(super) const LABEL: &str = "io.nats-fixture.diagnostics";
pub(super) const COLLECTION_BUDGET: Duration = Duration::from_secs(1);
pub(super) const MAX_INPUT: usize = 64 * 1024;
pub(super) const MAX_LINE: usize = 2048;
pub(super) const MAX_RECORDS: usize = 64;
