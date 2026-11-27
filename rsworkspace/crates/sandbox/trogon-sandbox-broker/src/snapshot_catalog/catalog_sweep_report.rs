#[derive(Debug, Default, Eq, PartialEq)]
pub struct CatalogSweepReport {
    pub pruned: usize,
    pub orphans_deleted: usize,
}
