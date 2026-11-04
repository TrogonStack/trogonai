use trogon_peer::{AttachmentRef, ConnectorId, NativeSessionId, PeerId};
use trogonai_proto::peers::v1alpha1 as v1;

use crate::PeerMcpError;
use crate::connector_config::ConnectorConfig;

pub(crate) struct NativeBinding {
    native_session_id: NativeSessionId,
    attachment: Option<AttachmentRef>,
}

impl NativeBinding {
    pub(crate) fn new(native_session_id: NativeSessionId) -> Self {
        Self {
            native_session_id,
            attachment: None,
        }
    }

    pub(crate) fn native_session_id(&self) -> &NativeSessionId {
        &self.native_session_id
    }

    pub(crate) fn attachment(&self) -> Option<&AttachmentRef> {
        self.attachment.as_ref()
    }

    pub(crate) fn revalidate(
        &mut self,
        response: &v1::EnrollResponse,
        config: &ConnectorConfig,
    ) -> Result<AttachmentRef, PeerMcpError> {
        let peer_id = response.peer_id.as_deref().ok_or(PeerMcpError::InvalidEnrollment)?;
        let peer_id = PeerId::parse(peer_id).map_err(PeerMcpError::InvalidEnrollmentIdentity)?;
        let lease = response.attachment.as_option().ok_or(PeerMcpError::InvalidEnrollment)?;
        if lease.status.as_ref().and_then(|status| status.as_known())
            != Some(v1::AttachmentStatus::ATTACHMENT_STATUS_ATTACHED)
        {
            return Err(PeerMcpError::InvalidEnrollment);
        }
        let observed = lease.observed_at.as_option().ok_or(PeerMcpError::InvalidEnrollment)?;
        let expires = lease.expires_at.as_option().ok_or(PeerMcpError::InvalidEnrollment)?;
        for timestamp in [observed, expires] {
            if !(-62_135_596_800..=253_402_300_799).contains(&timestamp.seconds)
                || !(0..1_000_000_000).contains(&timestamp.nanos)
            {
                return Err(PeerMcpError::InvalidEnrollment);
            }
        }
        if (expires.seconds, expires.nanos) <= (observed.seconds, observed.nanos) {
            return Err(PeerMcpError::InvalidEnrollment);
        }
        let raw = lease.attachment.as_option().ok_or(PeerMcpError::InvalidEnrollment)?;
        if raw.peer_id.as_deref() != Some(peer_id.as_str())
            || raw.connector_id.as_deref() != Some(config.connector_id().as_str())
        {
            return Err(PeerMcpError::InvalidEnrollment);
        }
        let connector_id = raw.connector_id.as_deref().ok_or(PeerMcpError::InvalidEnrollment)?;
        let connector_id = ConnectorId::new(connector_id).map_err(PeerMcpError::InvalidEnrollmentIdentity)?;
        let generation = raw.generation.ok_or(PeerMcpError::InvalidEnrollment)?;
        let attachment =
            AttachmentRef::new(peer_id, connector_id, generation).map_err(PeerMcpError::InvalidEnrollmentIdentity)?;
        if self
            .attachment
            .as_ref()
            .is_some_and(|prior| prior.peer_id() != attachment.peer_id())
        {
            return Err(PeerMcpError::InvalidEnrollment);
        }
        self.attachment = Some(attachment.clone());
        Ok(attachment)
    }
}
