use crate::{
    attachment_input::attachment,
    peer_access::PeerAccess,
    peer_application::{ApplicationResult, PeerApplication},
    peer_application_error::PeerApplicationError,
    peer_clock::PeerClock,
    peer_operation::PeerOperation,
    scoped_peer_store::ScopedPeerStore,
};
use buffa::MessageField;
use trogon_decider_runtime::{CommandAuthorizer, CommandError, StreamAppend, StreamRead};
use trogon_peer::{AcknowledgeBatch, BatchId, MessageError, PeerId, TrustedPeerOwner};
use trogonai_proto::peers::v1alpha1 as proto;
impl<S, P, C, Content, Cache, Ids, Catalog> PeerApplication<S, P, C, Content, Cache, Ids, Catalog>
where
    S: StreamRead<PeerId> + StreamAppend<PeerId> + ScopedPeerStore,
    P: CommandAuthorizer<PeerAccess> + CommandAuthorizer<AcknowledgeBatch> + Send + Sync,
    C: PeerClock,
{
    pub async fn acknowledge_batch(
        &self,
        owner: &TrustedPeerOwner,
        request: proto::AcknowledgeBatchRequest,
    ) -> ApplicationResult<proto::AcknowledgeBatchResponse, S> {
        let reference = attachment(request.attachment_ref.as_option().ok_or(PeerApplicationError::Input)?)?;
        let batch_id = BatchId::new(request.batch_id.ok_or(PeerApplicationError::Input)?)?;
        self.authorize(owner, reference.peer_id(), PeerOperation::Acknowledge)?;
        let batch = match self
            .execute_message_with_builder(owner, self.clock.now(), |now| {
                AcknowledgeBatch::new(owner.clone(), reference.clone(), batch_id.clone(), now)
            })
            .await
        {
            Ok(state) => state
                .batches
                .iter()
                .find(|b| b.batch_id.as_deref() == Some(batch_id.as_str()))
                .cloned()
                .ok_or(PeerApplicationError::Scope)?,
            Err(PeerApplicationError::Message(CommandError::Decide(MessageError::AlreadyAcknowledged(batch)))) => {
                *batch
            }
            Err(error) => return Err(error),
        };
        Ok(proto::AcknowledgeBatchResponse {
            batch: MessageField::some(batch),
        })
    }
}
