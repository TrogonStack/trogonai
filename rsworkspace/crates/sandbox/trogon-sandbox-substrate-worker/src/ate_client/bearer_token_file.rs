use std::fs;
use std::path::PathBuf;

/// The path to a bearer token, read fresh from disk on every call rather than cached at
/// connect time, because Kubernetes rotates a pod's projected service-account token in
/// place.
#[derive(Debug, Clone)]
pub struct BearerTokenFile(PathBuf);

impl BearerTokenFile {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self(path.into())
    }

    /// The path Kubernetes projects a pod's own service-account token to.
    pub fn default_service_account() -> Self {
        Self::new("/var/run/secrets/kubernetes.io/serviceaccount/token")
    }

    pub(super) fn read(&self) -> std::io::Result<String> {
        Ok(fs::read_to_string(&self.0)?.trim().to_string())
    }
}
