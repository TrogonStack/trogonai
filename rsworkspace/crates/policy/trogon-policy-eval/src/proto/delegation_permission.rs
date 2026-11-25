use trogonai_proto::policy::delegation_permissions::v1alpha1 as proto;

use super::ProtoConversionError;
use crate::delegation_permission::DelegationPermission;
use crate::delegation_target::DelegationTarget;

impl TryFrom<&proto::DelegationPermissionPolicy> for DelegationPermission {
    type Error = ProtoConversionError;

    /// A target listed twice collapses into one, because the permission is a
    /// set and a repeat widens nothing.
    fn try_from(value: &proto::DelegationPermissionPolicy) -> Result<Self, Self::Error> {
        if value.permitted.is_empty() {
            return Err(ProtoConversionError::EmptyPolicy {
                policy: "DelegationPermissionPolicy",
            });
        }
        value
            .permitted
            .iter()
            .map(DelegationTarget::try_from)
            .collect::<Result<Self, _>>()
    }
}
