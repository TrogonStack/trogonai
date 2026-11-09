use std::sync::Arc;

pub(crate) trait MaterialStatus: std::fmt::Debug + Send + Sync {
    fn is_current(&self) -> bool;
}

#[derive(Debug, Clone)]
pub(crate) struct MaterialAuthority {
    status: Arc<dyn MaterialStatus>,
}

impl MaterialAuthority {
    pub fn new(status: Arc<dyn MaterialStatus>) -> Self {
        Self { status }
    }

    pub fn require_current(&self) -> Result<(), rustls::Error> {
        if self.status.is_current() {
            Ok(())
        } else {
            Err(rustls::Error::InvalidCertificate(
                rustls::CertificateError::ApplicationVerificationFailure,
            ))
        }
    }
}
