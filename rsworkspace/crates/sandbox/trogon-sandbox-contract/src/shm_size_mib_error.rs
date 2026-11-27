/// A number is not a `/dev/shm` size a lease may ask for.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
#[error("shm_size_mib must be between 1 and {max}, not {got}")]
pub struct ShmSizeMibError {
    pub max: u32,
    pub got: u32,
}
