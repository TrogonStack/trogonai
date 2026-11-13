#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UsdBudget(f64);

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("a USD budget must be finite and nonnegative")]
pub struct UsdBudgetError;

impl UsdBudget {
    pub fn new(value: f64) -> Result<Self, UsdBudgetError> {
        if !value.is_finite() || value < 0.0 {
            return Err(UsdBudgetError);
        }
        Ok(Self(value))
    }
    pub fn get(self) -> f64 {
        self.0
    }
}

#[cfg(test)]
#[path = "usd_budget_tests.rs"]
mod tests;
