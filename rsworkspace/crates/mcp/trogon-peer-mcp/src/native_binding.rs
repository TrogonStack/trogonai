use tokio::sync::watch;
use trogon_peer::{AttachmentRef, ConnectorId, NativeSessionId, PeerId};
use trogonai_proto::peers::v1alpha1 as v1;

use crate::PeerMcpError;
use crate::connector_config::ConnectorConfig;

pub(crate) struct NativeBinding {
    native_session_id: NativeSessionId,
    attachment: Option<watch::Sender<AttachmentRef>>,
    enrollment: EnrollmentState,
    detach_confirmed: bool,
}

enum EnrollmentState {
    Uncertain,
    Replied,
}

impl NativeBinding {
    pub(crate) fn new(native_session_id: NativeSessionId) -> Self {
        Self {
            native_session_id,
            attachment: None,
            enrollment: EnrollmentState::Uncertain,
            detach_confirmed: false,
        }
    }

    pub(crate) fn native_session_id(&self) -> &NativeSessionId {
        &self.native_session_id
    }

    pub(crate) fn attachment(&self) -> Option<AttachmentRef> {
        match self.enrollment {
            EnrollmentState::Uncertain => None,
            EnrollmentState::Replied => self.last_attachment(),
        }
    }

    pub(crate) fn attachment_subscription(&self) -> Option<watch::Receiver<AttachmentRef>> {
        self.attachment()?;
        self.attachment.as_ref().map(watch::Sender::subscribe)
    }

    pub(crate) fn enrollment_started(&mut self) {
        self.enrollment = EnrollmentState::Uncertain;
    }

    fn last_attachment(&self) -> Option<AttachmentRef> {
        self.attachment.as_ref().map(|attachment| attachment.borrow().clone())
    }

    pub(crate) fn detach_confirmed(&self) -> bool {
        self.detach_confirmed
    }

    pub(crate) fn confirm_detach(&mut self) {
        self.detach_confirmed = true;
    }

    pub(crate) fn revalidate(
        &mut self,
        response: &v1::EnrollResponse,
        config: &ConnectorConfig,
    ) -> Result<AttachmentRef, PeerMcpError> {
        self.enrollment = EnrollmentState::Replied;
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
        let raw = lease
            .attachment_ref
            .as_option()
            .ok_or(PeerMcpError::InvalidEnrollment)?;
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
        if self.last_attachment().as_ref().is_some_and(|prior| {
            prior.peer_id() != attachment.peer_id() || prior.generation() > attachment.generation()
        }) {
            return Err(PeerMcpError::InvalidEnrollment);
        }
        match &self.attachment {
            Some(current) => {
                current.send_replace(attachment.clone());
            }
            None => self.attachment = Some(watch::channel(attachment.clone()).0),
        }
        Ok(attachment)
    }
}
