use trogonai_proto::policy::delegation_permissions::v1alpha1 as proto;
use trogonai_proto::policy::delegation_permissions::v1alpha1::__buffa::oneof::delegation_target::Target as Arm;

use super::ProtoConversionError;
use crate::agent_id::AgentId;
use crate::delegation_target::DelegationTarget;
use crate::node_id::NodeId;

impl TryFrom<&proto::DelegationTarget> for DelegationTarget {
    type Error = ProtoConversionError;

    fn try_from(value: &proto::DelegationTarget) -> Result<Self, Self::Error> {
        match value.target.as_ref() {
            Some(Arm::AgentId(id)) => Ok(Self::Agent(AgentId::try_from(id.as_ref())?)),
            Some(Arm::AgentsUnder(id)) => Ok(Self::AgentsUnder(NodeId::try_from(id.as_ref())?)),
            None => Err(ProtoConversionError::MissingField {
                message: "DelegationTarget",
                field: "target",
            }),
        }
    }
}
