use trogonai_proto::policy::model_ceilings::v1alpha1 as proto;

use super::ProtoConversionError;
use crate::model_ceiling::ModelCeiling;
use crate::model_id::ModelId;

impl TryFrom<&proto::ModelCeilingPolicy> for ModelCeiling {
    type Error = ProtoConversionError;

    /// A model listed twice collapses into one, because the ceiling is a set.
    ///
    /// An empty list is refused even though the domain type can hold an empty
    /// ceiling that admits no model: the schema calls that state invalid and
    /// withdraws a ceiling by clearing it, so it is never read as "admits
    /// nothing".
    fn try_from(value: &proto::ModelCeilingPolicy) -> Result<Self, Self::Error> {
        if value.within.is_empty() {
            return Err(ProtoConversionError::EmptyPolicy {
                policy: "ModelCeilingPolicy",
            });
        }
        value.within.iter().map(ModelId::try_from).collect::<Result<Self, _>>()
    }
}
