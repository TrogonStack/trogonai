/// Which quantity a [`crate::policy::tier2_dynamic::WindowedBudget`] tracks
/// for a `token_count_per_window` or `cost_per_window` dynamic condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum BudgetMetric {
    TokenCount,
    Cost,
}
