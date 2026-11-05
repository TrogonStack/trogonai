use trogonai_proto::agents::agents::v1alpha1 as proto;

use super::ProtoConversionError;
use crate::agent_id::AgentId;

impl TryFrom<&proto::AgentId> for AgentId {
    type Error = ProtoConversionError;

    fn try_from(value: &proto::AgentId) -> Result<Self, Self::Error> {
        Self::new(value.value.as_str()).map_err(ProtoConversionError::InvalidAgentId)
    }
}
