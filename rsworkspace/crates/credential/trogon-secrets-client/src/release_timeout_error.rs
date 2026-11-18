use std::time::Duration;

/// Why a wait cannot bound a release call.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum ReleaseTimeoutError {
    #[error("release timeout must be longer than zero")]
    Zero,
    #[error("release timeout of {requested:?} is longer than the {max:?} maximum")]
    TooLong { requested: Duration, max: Duration },
}
