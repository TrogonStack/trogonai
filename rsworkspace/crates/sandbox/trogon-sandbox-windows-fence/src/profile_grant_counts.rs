/// What a recorded grant wrote on, by kind.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ProfileGrantCounts {
    pub containers: u64,
    pub granted: u64,
    pub files: u64,
    pub denied: u64,
}
