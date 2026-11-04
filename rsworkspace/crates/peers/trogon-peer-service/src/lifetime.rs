use crate::constants::{CONFLICT_RETRY_LIMIT, PEER_REPLAY_LIMIT};
use crate::scoped_peer_store::ScopedPeerStore;
use crate::{
    attachment_input::attachment, peer_application_error::PeerApplicationError, peer_clock::PeerClock,
    peer_service::PeerService,
};
use trogon_decider_runtime::{
    CommandAuthorizer, CommandError, CommandExecution, CommandPrincipal, PrincipalId, PrincipalKind, StreamAppend,
    StreamRead,
};
use trogon_peer::{Detach, DetachError, Heartbeat, PeerId, PeerObservation, TrustedPeerOwner};
use trogonai_proto::{convert::datetime_from_timestamp, peers::v1alpha1 as proto};
impl<S, P, C> PeerService<S, P, C>
where
    S: StreamRead<PeerId> + StreamAppend<PeerId> + ScopedPeerStore,
    P: CommandAuthorizer<Heartbeat> + CommandAuthorizer<Detach> + Send + Sync,
    C: PeerClock,
{
    pub async fn heartbeat(
        &self,
        owner: &TrustedPeerOwner,
        request: proto::HeartbeatRequest,
    ) -> Result<
        proto::HeartbeatResponse,
        PeerApplicationError<<S as StreamRead<PeerId>>::Error, <S as StreamAppend<PeerId>>::Error>,
    > {
        if self.store.tenant() != owner.tenant() {
            return Err(PeerApplicationError::Scope);
        }
        let reference = attachment(request.attachment.as_option().ok_or(PeerApplicationError::Input)?)?;
        let availability = request
            .availability
            .and_then(|v| v.as_known())
            .ok_or(PeerApplicationError::Input)?;
        let condition = request
            .condition
            .map(|v| v.as_known().ok_or(PeerApplicationError::Input))
            .transpose()?;
        let active = request
            .active_at
            .as_option()
            .map(|at| datetime_from_timestamp(at).map_err(|_| PeerApplicationError::Input))
            .transpose()?;
        let command = Heartbeat::new(
            owner.clone(),
            reference,
            PeerObservation::new(availability, condition, active)?,
            self.clock.now(),
        );
        let principal = CommandPrincipal::new(PrincipalKind::Agent, PrincipalId::new(owner.caller().as_str())?);
        let result = CommandExecution::new(&self.store, &command)
            .with_principal(principal)
            .with_authorizer(&self.authorizer)
            .with_replay_limit(PEER_REPLAY_LIMIT)
            .with_conflict_retry(CONFLICT_RETRY_LIMIT)
            .execute()
            .await
            .map_err(PeerApplicationError::Heartbeat)?;
        Ok(proto::HeartbeatResponse {
            attachment: result.state.attachment,
        })
    }
    pub async fn detach(
        &self,
        owner: &TrustedPeerOwner,
        request: proto::DetachRequest,
    ) -> Result<
        proto::DetachResponse,
        PeerApplicationError<<S as StreamRead<PeerId>>::Error, <S as StreamAppend<PeerId>>::Error>,
    > {
        if self.store.tenant() != owner.tenant() {
            return Err(PeerApplicationError::Scope);
        }
        let command = Detach::new(
            owner.clone(),
            attachment(request.attachment.as_option().ok_or(PeerApplicationError::Input)?)?,
            self.clock.now(),
        );
        let principal = CommandPrincipal::new(PrincipalKind::Agent, PrincipalId::new(owner.caller().as_str())?);
        match CommandExecution::new(&self.store, &command)
            .with_principal(principal)
            .with_authorizer(&self.authorizer)
            .with_replay_limit(PEER_REPLAY_LIMIT)
            .with_conflict_retry(CONFLICT_RETRY_LIMIT)
            .execute()
            .await
        {
            Ok(_) | Err(CommandError::Decide(DetachError::AlreadyDetached)) => Ok(proto::DetachResponse {}),
            Err(error) => Err(PeerApplicationError::Detach(error)),
        }
    }
}
