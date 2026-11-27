/// What one `EntityShadow::repair` pass changed.
#[derive(Debug, Default, Clone, Copy, Eq, PartialEq)]
pub struct RepairReport {
    pub runs_ended: usize,
    pub runs_confirmed: usize,
    pub claims_removed: usize,
}
