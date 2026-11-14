use crate::{AuthenticationMode, EndpointId, EndpointStatus, ProjectId, TimestampTolerance, WebhookBodySize};

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
}
