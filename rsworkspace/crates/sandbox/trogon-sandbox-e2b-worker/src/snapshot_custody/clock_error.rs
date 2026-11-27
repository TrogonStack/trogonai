use std::num::TryFromIntError;
use std::time::SystemTimeError;

#[derive(Debug, Clone, thiserror::Error)]
pub enum SnapshotClockError {
    #[error("system clock is before the Unix epoch")]
    BeforeEpoch(#[from] SystemTimeError),
    #[error("system clock exceeds the millisecond range")]
    OutOfRange(#[from] TryFromIntError),
}
