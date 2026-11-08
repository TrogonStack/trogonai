use crate::{
    attachment_input::attachment,
    catalog_ports::PeerCatalog,
    constants::DEFAULT_ROSTER_PAGE,
    peer_access::PeerAccess,
    peer_application::{ApplicationResult, PeerApplication},
    peer_application_error::PeerApplicationError,
    peer_clock::PeerClock,
    peer_operation::PeerOperation,
    scoped_peer_store::ScopedPeerStore,
    work_limit::WorkLimit,
};
use buffa::MessageField;
use trogon_decider_runtime::{CommandAuthorizer, StreamAppend, StreamRead};
use trogon_peer::{PeerId, TrustedPeerOwner};
use trogonai_proto::{
    convert::{datetime_from_timestamp, timestamp_from_datetime},
    peers::v1alpha1 as proto,
};
impl<S, P, C, Content, Cache, Ids, Catalog> PeerApplication<S, P, C, Content, Cache, Ids, Catalog>
where
    S: StreamRead<PeerId> + StreamAppend<PeerId> + ScopedPeerStore,
    P: CommandAuthorizer<PeerAccess> + Send + Sync,
    C: PeerClock,
    Catalog: PeerCatalog,
{
    pub async fn roster(
        &self,
        owner: &TrustedPeerOwner,
        request: proto::RosterRequest,
    ) -> ApplicationResult<proto::RosterResponse, S> {
        let reference = attachment(request.attachment_ref.as_option().ok_or(PeerApplicationError::Input)?)?;
        let limit = WorkLimit::new(request.page_size.filter(|n| *n != 0).unwrap_or(DEFAULT_ROSTER_PAGE) as usize)?;
        let after = request
            .page_token
            .filter(|v| !v.is_empty())
            .map(PeerId::parse)
            .transpose()?;
        let scope = self.authorize(owner, reference.peer_id(), PeerOperation::Roster)?;
        let requester = self.load(owner, reference.peer_id()).await?;
        self.live(owner, &reference, &requester)?;
        let collaboration = self.collaboration(&requester)?;
        let members = self.catalog.page(&scope, &collaboration, after.as_ref(), limit).await?;
        if members.len() > limit.get()
            || members.windows(2).any(|p| p[0].as_str() >= p[1].as_str())
            || members
                .first()
                .is_some_and(|id| after.as_ref().is_some_and(|a| id.as_str() <= a.as_str()))
        {
            return Err(PeerApplicationError::Scope);
        }
        let next_page_token = if members.len() == limit.get() {
            members.last().map(|p| p.as_str().to_owned())
        } else {
            None
        };
        let now = self.clock.now();
        let mut peers = Vec::new();
        for peer in members {
            match self.authorize(owner, &peer, PeerOperation::ReadPeer) {
                Ok(_) => {}
                Err(PeerApplicationError::Denied(_)) => continue,
                Err(error) => return Err(error),
            }
            let state = self.load(owner, &peer).await?;
            if self.collaboration(&state)? != collaboration {
                return Err(PeerApplicationError::Scope);
            }
            let expires = datetime_from_timestamp(
                state
                    .attachment
                    .expires_at
                    .as_option()
                    .ok_or(PeerApplicationError::Scope)?,
            )
            .map_err(|_| PeerApplicationError::Scope)?;
            let live = state.attachment.status.and_then(|v| v.as_known())
                == Some(proto::AttachmentStatus::ATTACHMENT_STATUS_ATTACHED)
                && now < expires;
            peers.push(proto::PeerPresence {
                peer_id: state.enrollment.peer_id.clone(),
                display_name: state.enrollment.display_name.clone(),
                availability: if live { state.availability } else { None },
                condition: if live { state.condition } else { None },
                delivery_capability: Some(proto::DeliveryCapability::DELIVERY_CAPABILITY_ON_USER_PROMPT.into()),
                observed_at: MessageField::some(timestamp_from_datetime(&now)),
                expires_at: state.attachment.expires_at.clone(),
                active_at: state.active_at,
            });
        }
        Ok(proto::RosterResponse { peers, next_page_token })
    }
}
