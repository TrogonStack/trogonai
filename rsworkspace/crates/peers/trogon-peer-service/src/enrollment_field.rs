#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EnrollmentField {
    InstallationId,
    NativeSessionId,
    ConnectorId,
    CollaborationRef,
    DisplayName,
}

impl std::fmt::Display for EnrollmentField {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::InstallationId => "installation_id",
            Self::NativeSessionId => "native_session_id",
            Self::ConnectorId => "connector_id",
            Self::CollaborationRef => "collaboration_ref",
            Self::DisplayName => "display_name",
        })
    }
}
