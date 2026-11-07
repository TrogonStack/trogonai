use trogonai_proto::policy::model_ceilings::v1alpha1 as proto;
use trogonai_proto::policy::model_ceilings::v1alpha1::__buffa::oneof::model_matcher::Matcher as Arm;

use super::ProtoConversionError;
use crate::model_id::ModelId;

impl TryFrom<&proto::ModelMatcher> for ModelId {
    type Error = ProtoConversionError;

    fn try_from(value: &proto::ModelMatcher) -> Result<Self, Self::Error> {
        match value.matcher.as_ref() {
            Some(Arm::ModelId(id)) => Self::new(id.as_str()).map_err(ProtoConversionError::InvalidModelId),
            None => Err(ProtoConversionError::MissingField {
                message: "ModelMatcher",
                field: "matcher",
            }),
        }
    }
}
