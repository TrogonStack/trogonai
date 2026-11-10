use crate::constants::PEER_REPLAY_LIMIT;
use trogon_decider_runtime::{
    CommandAuthorizer, CommandExecution, CommandPrincipal, PrincipalId, PrincipalKind, StreamAppend, StreamRead,
};
use trogon_peer::{
    Attach, AttachError, CollaborationRef, ConnectorId, Enroll, EnrollmentIdentity, InstallationId, NativeSessionId,
    PeerDisplayName, PeerId, TrustedPeerOwner,
};
use trogonai_proto::peers::v1alpha1;

use crate::{EnrollResult, EnrollServiceError, EnrollmentField, PeerClock};

pub struct PeerService<Store, Authorizer, Clock> {
    pub(crate) store: Store,
    pub(crate) authorizer: Authorizer,
    pub(crate) clock: Clock,
}

impl<Store, Authorizer, Clock> PeerService<Store, Authorizer, Clock>
where
    Store: StreamRead<PeerId> + StreamAppend<PeerId>,
    Authorizer: CommandAuthorizer<Enroll> + CommandAuthorizer<Attach> + Send + Sync,
    Clock: PeerClock,
{
    pub fn new(store: Store, authorizer: Authorizer, clock: Clock) -> Self {
        Self {
            store,
            authorizer,
            clock,
        }
    }

    pub async fn enroll(
        &self,
        owner: &TrustedPeerOwner,
        request: v1alpha1::EnrollRequest,
    ) -> EnrollResult<<Store as StreamRead<PeerId>>::Error, <Store as StreamAppend<PeerId>>::Error> {
        let installation = InstallationId::new(request.installation_id.ok_or(EnrollServiceError::MissingInput {
            field: EnrollmentField::InstallationId,
        })?)
        .map_err(|source| EnrollServiceError::InvalidInput {
            field: EnrollmentField::InstallationId,
            source,
        })?;
        let native_session =
            NativeSessionId::new(request.native_session_id.ok_or(EnrollServiceError::MissingInput {
                field: EnrollmentField::NativeSessionId,
            })?)
            .map_err(|source| EnrollServiceError::InvalidInput {
                field: EnrollmentField::NativeSessionId,
                source,
            })?;
        let connector = ConnectorId::new(request.connector_id.ok_or(EnrollServiceError::MissingInput {
            field: EnrollmentField::ConnectorId,
        })?)
        .map_err(|source| EnrollServiceError::InvalidInput {
            field: EnrollmentField::ConnectorId,
            source,
        })?;
        let collaboration =
            CollaborationRef::new(request.collaboration_ref.ok_or(EnrollServiceError::MissingInput {
                field: EnrollmentField::CollaborationRef,
            })?)
            .map_err(|source| EnrollServiceError::InvalidInput {
                field: EnrollmentField::CollaborationRef,
                source,
            })?;
        let display_name = PeerDisplayName::new(request.display_name.ok_or(EnrollServiceError::MissingInput {
            field: EnrollmentField::DisplayName,
        })?)
        .map_err(|source| EnrollServiceError::InvalidInput {
            field: EnrollmentField::DisplayName,
            source,
        })?;
        let identity = EnrollmentIdentity::new(owner.clone(), installation, native_session);
        let command = Enroll::new(identity, connector, collaboration, display_name, self.clock.now());
        let principal = CommandPrincipal::new(PrincipalKind::Agent, PrincipalId::new(owner.caller().as_str())?);
        let result = CommandExecution::new(&self.store, &command)
            .with_principal(principal.clone())
            .with_authorizer(&self.authorizer)
            .execute()
            .await;
        let state = match result {
            Ok(result) => result.state,
            Err(error)
                if matches!(
                    &error,
                    trogon_decider_runtime::CommandError::Append(source)
                        if self.store.classify_append_failure(source) == trogon_decider_runtime::AppendFailure::WriteConflict
                ) =>
            {
                let attach = Attach::new(
                    command.identity().clone(),
                    command.connector_id().clone(),
                    command.collaboration().clone(),
                    self.clock.now(),
                );
                self.authorizer
                    .authorize_execution(Some(&principal), &attach)
                    .map_err(|error| {
                        EnrollServiceError::Attach(trogon_decider_runtime::CommandError::Unauthorized(error))
                    })?;
                let (_, validated_position) =
                    crate::history::load_with_position(&self.store, command.identity().peer_id())
                        .await
                        .map_err(EnrollServiceError::History)?;
                match CommandExecution::new(&self.store, &attach)
                    .with_expected_revision(validated_position)
                    .with_replay_limit(PEER_REPLAY_LIMIT)
                    .with_principal(principal)
                    .with_authorizer(&self.authorizer)
                    .execute()
                    .await
                {
                    Ok(result) => result.state,
                    Err(trogon_decider_runtime::CommandError::Decide(AttachError::AlreadyAttached)) => {
                        let current = crate::history::load(&self.store, command.identity().peer_id())
                            .await
                            .map_err(EnrollServiceError::History)?;
                        let expected = trogon_peer::AttachmentRef::new(
                            command.identity().peer_id().clone(),
                            command.connector_id().clone(),
                            current.attachment.attachment_ref.generation.unwrap_or(0),
                        )
                        .map_err(EnrollServiceError::ResumeReference)?;
                        trogon_peer::guard_live_attachment(
                            &current,
                            command.identity().owner(),
                            &expected,
                            self.clock.now(),
                        )
                        .map_err(EnrollServiceError::ResumeGuard)?;
                        current
                    }
                    Err(error) => return Err(EnrollServiceError::Attach(error)),
                }
            }
            Err(error) => return Err(EnrollServiceError::Execution(error)),
        };
        Ok(v1alpha1::EnrollResponse {
            peer_id: state.enrollment.peer_id.clone(),
            attachment: state.attachment,
        })
    }
}
