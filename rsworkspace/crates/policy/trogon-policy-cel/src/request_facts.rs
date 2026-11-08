use std::collections::BTreeMap;

use cel_interpreter::Context;

use crate::cel_limits::CelLimits;
use crate::fact_name::FactName;
use crate::fact_value::FactValue;
use crate::facts_limit_error::FactsLimitError;

/// What the rules may read about one request, as named values.
///
/// The type is neutral: it knows nothing about gateways, agents or models. The
/// host decides which facts a request has and what to call them, such as
/// `request`, `caller` and `headers`, and the rules are written against those
/// names. A fact the host did not supply is a fact the rule cannot read, and a
/// rule that reads it is refused.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RequestFacts {
    variables: BTreeMap<FactName, FactValue>,
}

impl RequestFacts {
    /// Starts with no facts.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a fact. A later fact with the same name replaces the earlier one.
    pub fn with(mut self, name: FactName, value: impl Into<FactValue>) -> Self {
        self.variables.insert(name, value.into());
        self
    }

    /// The fact with this name.
    pub fn get(&self, name: &str) -> Option<&FactValue> {
        self.variables.get(name)
    }

    /// How many facts there are.
    pub fn len(&self) -> usize {
        self.variables.len()
    }

    /// Returns whether there are no facts.
    pub fn is_empty(&self) -> bool {
        self.variables.is_empty()
    }

    /// Checks the facts against the input limits.
    pub(crate) fn check(&self, limits: &CelLimits) -> Result<(), FactsLimitError> {
        let mut bytes = 0;
        for (name, value) in &self.variables {
            bytes += name.as_str().len();
            value.charge(limits, 1, &mut bytes)?;
        }
        Ok(())
    }

    /// Binds every fact in a fresh interpreter context. Call after
    /// [`Self::check`] has accepted the facts.
    pub(crate) fn to_context(&self) -> Context<'static> {
        let mut context = Context::default();
        for (name, value) in &self.variables {
            context.add_variable_from_value(name.as_str(), value.to_cel());
        }
        context
    }
}
