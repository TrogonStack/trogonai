use trogonai_proto::policy::egress_permissions::v1alpha1 as proto;

use super::ProtoConversionError;
use crate::egress_destination::EgressDestination;
use crate::egress_permission::EgressPermission;

impl TryFrom<&proto::EgressPermissionPolicy> for EgressPermission {
    type Error = ProtoConversionError;

    /// A destination listed twice collapses into one, because the permission
    /// is a set and a repeat widens nothing.
    fn try_from(value: &proto::EgressPermissionPolicy) -> Result<Self, Self::Error> {
        if value.permitted.is_empty() {
            return Err(ProtoConversionError::EmptyPolicy {
                policy: "EgressPermissionPolicy",
            });
        }
        value
            .permitted
            .iter()
            .map(EgressDestination::try_from)
            .collect::<Result<Self, _>>()
    }
}
