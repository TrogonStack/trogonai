use buffa::Message as _;
use trogonai_proto::gateway::webhooks::v1;

use crate::{
    AuthenticationMode, EndpointId, EndpointIdError, EndpointStatus, ProjectId, ProjectIdError, TimestampTolerance,
    TimestampToleranceError, WebhookBodySize, WebhookBodySizeError,
};

#[derive(Debug, thiserror::Error)]
pub enum EndpointRegistrationError {
    #[error("invalid endpoint id")]
    EndpointId(#[source] EndpointIdError),
    #[error("invalid project id")]
    ProjectId(#[source] ProjectIdError),
    #[error("unsupported endpoint status {0}")]
    Status(i32),
    #[error("unsupported authentication mode {0}")]
    AuthenticationMode(i32),
    #[error("invalid webhook body limit")]
    BodySize(#[source] WebhookBodySizeError),
    #[error("invalid timestamp tolerance")]
    TimestampTolerance(#[source] TimestampToleranceError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndpointRegistration {
    endpoint_id: EndpointId,
    project_id: ProjectId,
    status: EndpointStatus,
    authentication_mode: AuthenticationMode,
    max_body_size: WebhookBodySize,
    timestamp_tolerance: TimestampTolerance,
}

impl EndpointRegistration {
    pub fn new(
        endpoint_id: EndpointId,
        project_id: ProjectId,
        status: EndpointStatus,
        authentication_mode: AuthenticationMode,
        max_body_size: WebhookBodySize,
        timestamp_tolerance: TimestampTolerance,
    ) -> Self {
        Self {
            endpoint_id,
            project_id,
            status,
            authentication_mode,
            max_body_size,
            timestamp_tolerance,
        }
    }

    pub fn endpoint_id(&self) -> &EndpointId {
        &self.endpoint_id
    }
    pub fn project_id(&self) -> &ProjectId {
        &self.project_id
    }
    pub fn status(&self) -> EndpointStatus {
        self.status
    }
    pub fn authentication_mode(&self) -> AuthenticationMode {
        self.authentication_mode
    }
    pub fn max_body_size(&self) -> WebhookBodySize {
        self.max_body_size
    }
    pub fn timestamp_tolerance(&self) -> TimestampTolerance {
        self.timestamp_tolerance
    }

    pub fn encode(&self) -> Vec<u8> {
        v1::EndpointRegistration::from(self).encode_to_vec()
    }
}

impl From<&EndpointRegistration> for v1::EndpointRegistration {
    fn from(registration: &EndpointRegistration) -> Self {
        Self {
            endpoint_id: registration.endpoint_id().as_str().to_owned(),
            project_id: registration.project_id().as_str().to_owned(),
            status: match registration.status() {
                EndpointStatus::Enabled => v1::EndpointStatus::Enabled,
                EndpointStatus::Disabled => v1::EndpointStatus::Disabled,
            }
            .into(),
            authentication_mode: match registration.authentication_mode() {
                AuthenticationMode::Bearer => v1::AuthenticationMode::Bearer,
                AuthenticationMode::HmacSha256 => v1::AuthenticationMode::HmacSha256,
            }
            .into(),
            max_body_bytes: registration.max_body_size().get() as u32,
            timestamp_tolerance_seconds: registration.timestamp_tolerance().as_secs() as u32,
        }
    }
}

impl TryFrom<v1::EndpointRegistration> for EndpointRegistration {
    type Error = EndpointRegistrationError;

    fn try_from(wire: v1::EndpointRegistration) -> Result<Self, Self::Error> {
        let status = match wire.status.as_known() {
            Some(v1::EndpointStatus::Enabled) => EndpointStatus::Enabled,
            Some(v1::EndpointStatus::Disabled) => EndpointStatus::Disabled,
            _ => return Err(EndpointRegistrationError::Status(wire.status.to_i32())),
        };
        let authentication_mode = match wire.authentication_mode.as_known() {
            Some(v1::AuthenticationMode::Bearer) => AuthenticationMode::Bearer,
            Some(v1::AuthenticationMode::HmacSha256) => AuthenticationMode::HmacSha256,
            _ => {
                return Err(EndpointRegistrationError::AuthenticationMode(
                    wire.authentication_mode.to_i32(),
                ));
            }
        };
        Ok(Self::new(
            EndpointId::new(&wire.endpoint_id).map_err(EndpointRegistrationError::EndpointId)?,
            ProjectId::new(&wire.project_id).map_err(EndpointRegistrationError::ProjectId)?,
            status,
            authentication_mode,
            WebhookBodySize::new(wire.max_body_bytes as usize).map_err(EndpointRegistrationError::BodySize)?,
            TimestampTolerance::new(u64::from(wire.timestamp_tolerance_seconds))
                .map_err(EndpointRegistrationError::TimestampTolerance)?,
        ))
    }
}
