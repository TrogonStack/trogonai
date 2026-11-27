use crate::ate_client::AteError;

#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum TemplateError {
    /// The golden build failed, or did not finish inside the deadline. Carries the reason
    /// Substrate gave so the tenant sees why their image did not start.
    #[error("image {image} is not ready: {reason}")]
    NotReady { image: String, reason: String },
    /// The control plane refused in a way that is not about this image.
    #[error("substrate template error: {message}")]
    Backend { message: String },
}

impl From<AteError> for TemplateError {
    fn from(error: AteError) -> Self {
        Self::Backend {
            message: error.to_string(),
        }
    }
}
