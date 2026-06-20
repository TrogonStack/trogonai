use trogonai_session_kernel::SessionKernelConfig;

/// Default preview size for claim-check artifacts (512 bytes).
pub const DEFAULT_PREVIEW_MAX_BYTES: usize = 512;
/// Default artifact retention policy scope label.
pub const DEFAULT_PERMISSION_SCOPE: &str = "workspace:default";

/// Artifact store configuration derived from session kernel settings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArtifactStoreConfig {
    pub inline_limit_bytes: usize,
    pub preview_max_bytes: usize,
    pub bucket_name: String,
    pub permission_scope: String,
}

impl ArtifactStoreConfig {
    pub fn from_session_kernel(config: &SessionKernelConfig) -> Self {
        Self {
            inline_limit_bytes: config.inline_artifact_limit_bytes,
            preview_max_bytes: DEFAULT_PREVIEW_MAX_BYTES,
            bucket_name: session_artifacts_bucket(&config.nats_prefix),
            permission_scope: DEFAULT_PERMISSION_SCOPE.to_string(),
        }
    }
}

/// NATS Object Store bucket for durable session artifacts.
pub fn session_artifacts_bucket(prefix: &str) -> String {
    format!("{prefix}_SESSION_ARTIFACTS")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bucket_name_uses_acp_prefix() {
        assert_eq!(session_artifacts_bucket("ACP"), "ACP_SESSION_ARTIFACTS");
    }

    #[test]
    fn config_derives_from_session_kernel_defaults() {
        let kernel_config = SessionKernelConfig::default();
        let config = ArtifactStoreConfig::from_session_kernel(&kernel_config);
        assert_eq!(config.inline_limit_bytes, kernel_config.inline_artifact_limit_bytes);
        assert_eq!(config.bucket_name, "ACP_SESSION_ARTIFACTS");
        assert_eq!(config.preview_max_bytes, DEFAULT_PREVIEW_MAX_BYTES);
    }
}
