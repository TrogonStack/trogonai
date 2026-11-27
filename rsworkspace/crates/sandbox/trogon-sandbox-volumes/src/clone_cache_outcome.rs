#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum CloneCacheOutcome {
    Hit,
    Miss,
}
