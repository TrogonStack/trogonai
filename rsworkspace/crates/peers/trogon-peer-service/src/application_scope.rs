use crate::{
    constants::{CONFLICT_RETRY_LIMIT, PEER_REPLAY_LIMIT},
    execution_error::ExecutionError,
    peer_access::PeerAccess,
    peer_application::{ApplicationResult, PeerApplication},
    peer_application_error::PeerApplicationError,
    peer_clock::PeerClock,
    peer_operation::PeerOperation,
    scoped_peer_store::ScopedPeerStore,
};
use trogon_decider_runtime::{
    CommandAuthorizer, CommandExecution, CommandPrincipal, Decider, PrincipalId, PrincipalKind, StreamAppend,
    StreamRead,
};
use trogon_peer::{AttachmentRef, CollaborationRef, MessageError, PeerId, TrustedPeerOwner, guard_live_attachment};
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
    pub(crate) async fn load(&self, owner: &TrustedPeerOwner, peer: &PeerId) -> ApplicationResult<PeerState, S> {
        let state = crate::history::load(&self.store, peer).await?;
        if state.enrollment.peer_id.as_deref() != Some(peer.as_str())
            || state.enrollment.tenant_id.as_deref() != Some(owner.tenant().as_str())
        {
            return Err(PeerApplicationError::Scope);
        }
        Ok(state)
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
    pub(crate) fn collaboration(&self, state: &PeerState) -> ApplicationResult<CollaborationRef, S> {
        Ok(CollaborationRef::new(
            state
                .enrollment
                .collaboration_ref
                .clone()
                .ok_or(PeerApplicationError::Scope)?,
        )?)
    }
    pub(crate) async fn execute_message<Cmd>(
        &self,
        owner: &TrustedPeerOwner,
        command: &Cmd,
    ) -> Result<
        PeerState,
        ExecutionError<MessageError, <S as StreamRead<PeerId>>::Error, <S as StreamAppend<PeerId>>::Error>,
    >
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
        let principal = CommandPrincipal::new(
            PrincipalKind::Agent,
            PrincipalId::new(owner.caller().as_str()).map_err(|_| {
                trogon_decider_runtime::CommandError::Unauthorized(trogon_decider_runtime::UnauthorizedError::Denied(
                    trogon_decider_runtime::AuthorizationDeniedError::new("invalid peer principal"),
                ))
            })?,
        );
        Ok(CommandExecution::new(&self.store, command)
            .with_principal(principal)
            .with_authorizer(&self.policy)
            .with_replay_limit(PEER_REPLAY_LIMIT)
            .with_conflict_retry(CONFLICT_RETRY_LIMIT)
            .execute()
            .await?
            .state)
    }
}
