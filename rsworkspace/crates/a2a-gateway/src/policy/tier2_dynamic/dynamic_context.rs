/// Runtime, per-call input for Tier-2 dynamic budget conditions
/// (`token_count_per_window`, `cost_per_window`), mirroring the spec's
/// `dynamic_context.budget.{token_count,cost}` fields.
///
/// Temporal conditions (`time_window`, `day_of_week`) need no runtime
/// input beyond the injected [`super::clock::Tier2Clock`], so this type
/// only carries budget amounts.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Tier2DynamicContext {
    budget_token_count: Option<f64>,
    budget_cost: Option<f64>,
}

impl Tier2DynamicContext {
    /// No budget consumed by this call; temporal-only conditions still
    /// evaluate normally, and budget conditions treat a missing amount
    /// as `0.0` consumed (see [`Self::budget_token_count`]).
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn with_budget_token_count(mut self, amount: f64) -> Self {
        self.budget_token_count = Some(amount);
        self
    }

    pub fn with_budget_cost(mut self, amount: f64) -> Self {
        self.budget_cost = Some(amount);
        self
    }

    pub fn budget_token_count(&self) -> f64 {
        self.budget_token_count.unwrap_or(0.0)
    }

    pub fn budget_cost(&self) -> f64 {
        self.budget_cost.unwrap_or(0.0)
    }
}
