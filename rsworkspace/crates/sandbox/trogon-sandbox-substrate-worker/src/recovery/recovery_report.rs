/// What a startup pass concluded.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct RecoveryReport {
    /// Leases put back into the active map.
    pub adopted: usize,
    /// Index entries dropped because the actor behind them is gone.
    pub pruned: usize,
    /// Entries left alone because they hold a named lease's state with no live lease.
    pub retained: usize,
}
