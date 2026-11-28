use trogonai_proto::evaluation::rubrics::v1alpha1::RubricVersionRef;

/// A rubric version reference that does not name one rubric version.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum RubricKeyError {
    #[error("rubric version reference has no rubric ID")]
    MissingRubricId,
    #[error("rubric version reference has no version")]
    MissingVersion,
}

/// One pinned rubric version. A finding category is meaningful only against the rubric that
/// defined it, so every count is scoped by this key.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct RubricKey {
    rubric_id: String,
    version: String,
}

impl RubricKey {
    pub fn new(rubric_id: impl Into<String>, version: impl Into<String>) -> Result<Self, RubricKeyError> {
        let rubric_id = rubric_id.into();
        if rubric_id.is_empty() {
            return Err(RubricKeyError::MissingRubricId);
        }
        let version = version.into();
        if version.is_empty() {
            return Err(RubricKeyError::MissingVersion);
        }
        Ok(Self { rubric_id, version })
    }
}

impl TryFrom<&RubricVersionRef> for RubricKey {
    type Error = RubricKeyError;

    fn try_from(reference: &RubricVersionRef) -> Result<Self, Self::Error> {
        let rubric_id = reference.rubric_id.as_option().ok_or(RubricKeyError::MissingRubricId)?;
        Self::new(rubric_id.value.as_str(), reference.version.as_str())
    }
}

#[cfg(test)]
mod tests;
