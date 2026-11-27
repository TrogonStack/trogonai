use std::time::Duration;

#[derive(Debug, Clone)]
pub struct CatalogSweepConfig {
    /// Snapshots kept per (tenant, source lease). Zero disables keep-N.
    pub keep_per_lease: usize,
    /// How long an artifact must lack a record before it counts as an orphan.
    pub orphan_grace: Duration,
}
