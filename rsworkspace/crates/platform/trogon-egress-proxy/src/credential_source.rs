use trogon_semconv::attribute::EgressProxyCredentialSource;

/// Where the listener's own identity and the client trust anchors came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CredentialSource {
    WorkloadApi,
    StaticFile,
}

impl CredentialSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::WorkloadApi => EgressProxyCredentialSource::WorkloadApi.as_str(),
            Self::StaticFile => EgressProxyCredentialSource::StaticFile.as_str(),
        }
    }
}
