use std::time::Duration;
use tokio::sync::RwLock;

pub(super) static BULK: RwLock<()> = RwLock::const_new(());

/// How long a case may wait on anything that is not under a deadline of its
/// own: far beyond a loaded machine, so that only a step that is stuck trips it.
pub(crate) const STEP: Duration = Duration::from_secs(300);

/// Past [`STEP`], because a case queues here behind every bulk transfer ahead
/// of it, each of which may take a step of its own.
pub(super) const LOCK: Duration = Duration::from_secs(1200);
