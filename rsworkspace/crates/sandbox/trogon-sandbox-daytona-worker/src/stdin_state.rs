use std::sync::atomic::AtomicBool;

use tokio::sync::Mutex;
use trogon_sandbox_protocol::StdinSequencer;

/// Ordering state of an interactive command's stdin. The sequencer lock is held across a whole write, so the writes
/// to one command reach it strictly in order.
#[derive(Debug, Default)]
pub(crate) struct StdinState {
    pub(crate) sequencer: Mutex<StdinSequencer>,
    /// Set once an input call failed partway, after which no write can be trusted.
    pub(crate) broken: AtomicBool,
}
