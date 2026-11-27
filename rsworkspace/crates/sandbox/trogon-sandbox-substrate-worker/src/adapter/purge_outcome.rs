/// Outcome of `SubstrateProvider::purge_named`.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum PurgeOutcome {
    Purged,
    NotFound,
}
