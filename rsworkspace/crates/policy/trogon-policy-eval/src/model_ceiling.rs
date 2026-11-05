use std::collections::BTreeSet;

use crate::model_id::ModelId;

/// The catalog models within which every model selection of a subject's
/// Sessions must fall, auxiliary roles included (ADR#0071 Decision 5).
///
/// A ceiling that exists and names no models admits no model. That is a
/// deliberate, recorded narrowing and is not the same as the absence of a
/// ceiling, which admits everything the connection already authorizes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModelCeiling {
    models: BTreeSet<ModelId>,
}

impl ModelCeiling {
    /// Builds a ceiling from the models it admits.
    pub fn new(models: impl IntoIterator<Item = ModelId>) -> Self {
        Self {
            models: models.into_iter().collect(),
        }
    }

    /// Returns whether the model is named exactly.
    pub fn admits(&self, model: &ModelId) -> bool {
        self.models.contains(model)
    }
}

impl FromIterator<ModelId> for ModelCeiling {
    fn from_iter<I: IntoIterator<Item = ModelId>>(models: I) -> Self {
        Self::new(models)
    }
}
