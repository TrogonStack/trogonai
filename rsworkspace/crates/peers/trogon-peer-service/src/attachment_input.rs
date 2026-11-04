use trogon_peer::{AttachmentRef, ConnectorId, PeerId, PeerValueError};
use trogonai_proto::peers::v1alpha1 as proto;
pub(crate) fn attachment(input: &proto::AttachmentRef) -> Result<AttachmentRef, PeerValueError> {
    AttachmentRef::new(
        PeerId::parse(input.peer_id.clone().unwrap_or_default())?,
        ConnectorId::new(input.connector_id.clone().unwrap_or_default())?,
        input.generation.unwrap_or(0),
    )
}
