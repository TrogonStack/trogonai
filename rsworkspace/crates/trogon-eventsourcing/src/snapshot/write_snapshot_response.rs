#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WriteSnapshotResponse;

impl WriteSnapshotResponse {
    pub const fn new() -> Self {
        Self
    }
}
