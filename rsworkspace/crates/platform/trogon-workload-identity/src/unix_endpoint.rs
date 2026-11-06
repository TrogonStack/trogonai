use spiffe::transport::Endpoint;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct UnixEndpoint(PathBuf);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("workload identity requires an explicit absolute Unix socket endpoint")]
pub struct UnixEndpointError;

impl UnixEndpoint {
    pub fn new(value: &str) -> Result<Self, UnixEndpointError> {
        match Endpoint::parse(value) {
            Ok(Endpoint::Unix(path)) => Ok(Self(path)),
            _ => Err(UnixEndpointError),
        }
    }

    pub fn endpoint(&self) -> Endpoint {
        Endpoint::Unix(self.0.clone())
    }
    pub fn as_uri(&self) -> String {
        format!("unix://{}", self.0.display())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn production_identity_refuses_tcp_even_when_sdk_accepts_it() {
        assert!(UnixEndpoint::new("tcp://127.0.0.1:8081").is_err());
        assert!(UnixEndpoint::new("unix:relative.sock").is_err());
        assert!(UnixEndpoint::new("unix:///run/identity/api.sock").is_ok());
    }
}
