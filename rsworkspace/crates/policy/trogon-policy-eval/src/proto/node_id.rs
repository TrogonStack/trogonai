use trogonai_proto::hierarchy::v1alpha1 as proto;

use super::ProtoConversionError;
use crate::node_id::NodeId;

impl TryFrom<&proto::NodeId> for NodeId {
    type Error = ProtoConversionError;

    fn try_from(value: &proto::NodeId) -> Result<Self, Self::Error> {
        Self::new(value.value.as_str()).map_err(ProtoConversionError::InvalidNodeId)
    }
}
