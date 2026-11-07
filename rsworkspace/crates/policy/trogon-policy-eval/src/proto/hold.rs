use trogonai_proto::policy::holds::v1alpha1 as proto;

use super::ProtoConversionError;
use crate::hold::Hold;
use crate::hold_name::HoldName;
use crate::hold_scope::HoldScope;

impl TryFrom<&proto::Hold> for Hold {
    type Error = ProtoConversionError;

    fn try_from(value: &proto::Hold) -> Result<Self, Self::Error> {
        let name = value.name.as_option().ok_or(ProtoConversionError::MissingField {
            message: "Hold",
            field: "name",
        })?;
        let scope = value
            .scope
            .as_known()
            .ok_or(ProtoConversionError::UnknownEnumValue {
                enumeration: "HoldScope",
                value: value.scope.to_i32(),
            })
            .and_then(HoldScope::try_from)?;
        Ok(Self::new(HoldName::try_from(name)?, scope))
    }
}
