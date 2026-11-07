use trogonai_proto::policy::subjects::v1alpha1 as proto;
use trogonai_proto::policy::subjects::v1alpha1::__buffa::oneof::policy_subject::Subject as Arm;

use super::ProtoConversionError;
use crate::agent_id::AgentId;
use crate::node_id::NodeId;
use crate::subject::Subject;

impl TryFrom<&proto::PolicySubject> for Subject {
    type Error = ProtoConversionError;

    fn try_from(value: &proto::PolicySubject) -> Result<Self, Self::Error> {
        match value.subject.as_ref() {
            Some(Arm::AgentId(id)) => Ok(Self::Agent(AgentId::try_from(id.as_ref())?)),
            Some(Arm::Node(id)) => Ok(Self::Node(NodeId::try_from(id.as_ref())?)),
            None => Err(ProtoConversionError::MissingField {
                message: "PolicySubject",
                field: "subject",
            }),
        }
    }
}
