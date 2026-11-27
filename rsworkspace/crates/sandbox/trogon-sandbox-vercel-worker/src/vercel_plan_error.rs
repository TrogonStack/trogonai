/// A Vercel plan name that is not one of the plans the worker knows.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
#[error("unknown Vercel plan {name:?}; expected hobby, pro or enterprise")]
pub struct VercelPlanError {
    name: String,
}

impl VercelPlanError {
    pub(crate) fn new(name: &str) -> Self {
        Self { name: name.to_owned() }
    }

    /// The rejected name, lowercased.
    pub fn name(&self) -> &str {
        &self.name
    }
}
