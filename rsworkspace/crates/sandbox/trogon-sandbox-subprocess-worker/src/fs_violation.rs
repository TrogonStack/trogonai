//! Filesystem denials a backend observed, published as `FsViolation` on the
//! same sink, rate limiter and subject family as the egress proxy's denials.

use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::mpsc::{Receiver, Sender};
use trogon_sandbox_contract::{FsOperation, LiteralPath};

#[cfg(unix)]
mod reporter;
#[cfg(unix)]
pub use reporter::FsViolationReporter;

/// What a backend saw refused, before it is attributed to a lease.
#[derive(Debug, Clone, Eq, PartialEq)]
pub struct FsDenial {
    pub path: LiteralPath,
    pub operation: FsOperation,
}

/// Why a denial did not reach the reporter: the lease's reporter is gone or too far behind.
#[derive(Debug, thiserror::Error)]
#[error("the filesystem denial was dropped: {0}")]
pub struct FsDenialSendError(String);

/// Where a backend reports the denials it observes for one lease.
///
/// The channel is bounded and a backend never waits on it: a denial that finds it full is
/// dropped, since the reporter rate limits per lease anyway.
#[derive(Debug, Clone)]
pub struct FsDenialSink(Sender<FsDenial>);

impl FsDenialSink {
    /// How many denials may wait for the reporter.
    pub const CAPACITY: usize = 1024;

    pub fn channel() -> (Self, Receiver<FsDenial>) {
        let (sender, receiver) = tokio::sync::mpsc::channel(Self::CAPACITY);
        (Self(sender), receiver)
    }

    pub fn send(&self, denial: FsDenial) -> Result<(), FsDenialSendError> {
        match self.0.try_send(denial) {
            Ok(()) | Err(TrySendError::Full(_)) => Ok(()),
            Err(TrySendError::Closed(_)) => Err(FsDenialSendError("the reporter has stopped".to_owned())),
        }
    }
}

#[cfg(all(test, unix))]
mod tests;
