use trogonai_proto::policy::holds::v1alpha1 as proto;

use super::ProtoConversionError;
use crate::hold_scope::HoldScope;

impl TryFrom<proto::HoldScope> for HoldScope {
    type Error = ProtoConversionError;

    fn try_from(value: proto::HoldScope) -> Result<Self, Self::Error> {
        match value {
            proto::HoldScope::HOLD_SCOPE_NEW_WORK => Ok(Self::NewWork),
            proto::HoldScope::HOLD_SCOPE_ALL_PROTECTED_ACTIONS => Ok(Self::AllProtectedActions),
            proto::HoldScope::HOLD_SCOPE_UNSPECIFIED => Err(ProtoConversionError::UnknownEnumValue {
                enumeration: "HoldScope",
                value: value as i32,
            }),
        }
    }
}
