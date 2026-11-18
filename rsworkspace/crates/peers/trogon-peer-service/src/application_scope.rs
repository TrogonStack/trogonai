use crate::{
    constants::{CONFLICT_RETRY_LIMIT, PEER_REPLAY_LIMIT},
    peer_access::PeerAccess,
    peer_application::{ApplicationResult, PeerApplication},
    peer_application_error::PeerApplicationError,
    peer_clock::PeerClock,
    peer_operation::PeerOperation,
    scoped_peer_history::ScopedPeerHistory,
    scoped_peer_store::ScopedPeerStore,
};
use chrono::{DateTime, Utc};
use trogon_decider_runtime::{
    CommandAuthorizer, CommandError, CommandExecution, CommandPrincipal, Decider, PrincipalId, PrincipalKind,
    StreamAppend, StreamRead,
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
        self.load_with_position(owner, peer).await.map(|(history, _)| history)
    }
    pub(crate) async fn load_with_position(
        &self,
        owner: &TrustedPeerOwner,
        peer: &PeerId,
    ) -> ApplicationResult<(ScopedPeerHistory, Option<trogon_decider_runtime::StreamPosition>), S> {
        let (history, position) = crate::history::load_peer_history_with_position(&self.store, peer).await?;
        let history = ScopedPeerHistory::from_history(history, owner, peer).ok_or(PeerApplicationError::Scope)?;
        Ok((history, position))
    }
    pub(crate) fn live(
        &self,
        owner: &TrustedPeerOwner,
        reference: &AttachmentRef,
        state: &PeerState,
    ) -> ApplicationResult<DateTime<Utc>, S> {
        let now = self.clock.now();
        guard_live_attachment(state, owner, reference, now)
            .map_err(|error| PeerApplicationError::Evidence(MessageError::Guard(error)))?;
        Ok(now)
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
        let mut retries_left = CONFLICT_RETRY_LIMIT.as_u32();
        loop {
            let result = self.execute_message_once(owner, command).await;
            if retries_left > 0
                && matches!(&result, Err(PeerApplicationError::Message(CommandError::Append(error)))
                    if self.store.classify_append_failure(error) == trogon_decider_runtime::AppendFailure::WriteConflict)
            {
                retries_left -= 1;
                continue;
            }
            return result;
        }
    }

    pub(crate) async fn execute_message_with_builder<Cmd, Build>(
        &self,
        owner: &TrustedPeerOwner,
        authorized_at: DateTime<Utc>,
        mut build: Build,
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
        Build: FnMut(DateTime<Utc>) -> Cmd,
    {
        let mut retries_left = CONFLICT_RETRY_LIMIT.as_u32();
        let mut validated_position = None;
        loop {
            let command = build(authorized_at);
            let principal = self.principal(owner)?;
            self.policy
                .authorize_execution(Some(&principal), &command)
                .map_err(|error| PeerApplicationError::Message(CommandError::Unauthorized(error)))?;
            let position = match validated_position.take() {
                Some(position) => position,
                None => {
                    crate::history::load_with_position(&self.store, command.stream_id())
                        .await?
                        .1
                }
            };
            let command = build(self.clock.now());
            let result = CommandExecution::new(&self.store, &command)
                .with_expected_revision(position)
                .with_principal(principal)
                .with_authorizer(&self.policy)
                .with_replay_limit(PEER_REPLAY_LIMIT)
                .execute()
                .await
                .map(|result| result.state)
                .map_err(PeerApplicationError::Message);
            if retries_left > 0
                && matches!(
                    &result,
                    Err(PeerApplicationError::Message(CommandError::Decide(
                        MessageError::Guard(trogon_peer::HeartbeatError::ClockRegression)
                    )))
                )
            {
                let (_, current_position) =
                    crate::history::load_with_position(&self.store, command.stream_id()).await?;
                if current_position > position {
                    validated_position = Some(current_position);
                    retries_left -= 1;
                    continue;
                }
            }
            if retries_left > 0
                && matches!(&result, Err(PeerApplicationError::Message(CommandError::Append(error)))
                    if self.store.classify_append_failure(error) == trogon_decider_runtime::AppendFailure::WriteConflict)
            {
                retries_left -= 1;
                continue;
            }
            return result;
        }
    }

    pub(crate) async fn execute_message_once<Cmd>(
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
        self.policy
            .authorize_execution(Some(&principal), command)
            .map_err(|error| PeerApplicationError::Message(CommandError::Unauthorized(error)))?;
        let (_, position) = crate::history::load_with_position(&self.store, command.stream_id()).await?;
        CommandExecution::new(&self.store, command)
            .with_expected_revision(position)
            .with_principal(principal)
            .with_authorizer(&self.policy)
            .with_replay_limit(PEER_REPLAY_LIMIT)
            .execute()
            .await
            .map(|result| result.state)
            .map_err(PeerApplicationError::Message)
    }
}
