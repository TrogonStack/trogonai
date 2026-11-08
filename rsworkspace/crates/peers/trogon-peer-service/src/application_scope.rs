use crate::{
    constants::PEER_REPLAY_LIMIT,
    peer_access::PeerAccess,
    peer_application::{ApplicationResult, PeerApplication},
    peer_application_error::PeerApplicationError,
    peer_clock::PeerClock,
    peer_operation::PeerOperation,
    scoped_peer_history::ScopedPeerHistory,
    scoped_peer_store::ScopedPeerStore,
};
use trogon_decider_runtime::{
    CommandAuthorizer, CommandExecution, CommandPrincipal, Decider, PrincipalId, PrincipalKind, StreamAppend,
    StreamRead,
};
use trogon_peer::{AttachmentRef, MessageError, PeerId, TrustedPeerOwner, guard_live_attachment};
use trogonai_proto::peers::{state_v1alpha1::PeerState, v1alpha1 as proto};
impl<S, P, C, Content, Cache, Ids, Catalog> PeerApplication<S, P, C, Content, Cache, Ids, Catalog>
where
    S: StreamRead<PeerId> + StreamAppend<PeerId> + ScopedPeerStore,
    P: CommandAuthorizer<PeerAccess> + Send + Sync,
    C: PeerClock,
{
    pub(crate) fn principal(&self, owner: &TrustedPeerOwner) -> ApplicationResult<CommandPrincipal, S> {
        Ok(CommandPrincipal::new(
            PrincipalKind::Agent,
            PrincipalId::new(owner.caller().as_str())?,
        ))
    }
    pub(crate) fn authorize(
        &self,
        owner: &TrustedPeerOwner,
        peer: &PeerId,
        operation: PeerOperation,
    ) -> ApplicationResult<PeerAccess, S> {
        let scope = PeerAccess::new(owner.clone(), peer.clone(), operation);
        self.policy.authorize(&self.principal(owner)?, &scope)?;
        if self.store.tenant() != owner.tenant() {
            return Err(PeerApplicationError::Scope);
        }
        Ok(scope)
    }
    pub(crate) async fn load(
        &self,
        owner: &TrustedPeerOwner,
        peer: &PeerId,
    ) -> ApplicationResult<ScopedPeerHistory, S> {
        let (history, _) = crate::history::load_peer_history_with_position(&self.store, peer).await?;
        ScopedPeerHistory::from_history(history, owner, peer).ok_or(PeerApplicationError::Scope)
    }
    pub(crate) fn live(
        &self,
        owner: &TrustedPeerOwner,
        reference: &AttachmentRef,
        state: &PeerState,
    ) -> ApplicationResult<(), S> {
        guard_live_attachment(state, owner, reference, self.clock.now())
            .map_err(|error| PeerApplicationError::Evidence(MessageError::Guard(error)))?;
        Ok(())
    }
    pub(crate) async fn execute_message<Cmd>(
        &self,
        owner: &TrustedPeerOwner,
        command: &Cmd,
    ) -> ApplicationResult<PeerState, S>
    where
        Cmd: Decider<
                StreamId = PeerId,
                State = PeerState,
                Event = proto::PeerEvent,
                DecideError = MessageError,
                EvolveError = trogon_peer::EvolveError,
            > + Sync,
        P: CommandAuthorizer<Cmd>,
    {
        let principal = self.principal(owner)?;
        Ok(CommandExecution::new(&self.store, command)
            .with_principal(principal)
            .with_authorizer(&self.policy)
            .with_replay_limit(PEER_REPLAY_LIMIT)
            .execute()
            .await
            .map_err(PeerApplicationError::Message)?
            .state)
    }
}
