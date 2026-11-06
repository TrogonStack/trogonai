use spiffe::transport::Endpoint;

#[derive(Debug, Clone)]
pub struct UnixEndpoint(Endpoint);

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("workload identity requires an explicit absolute Unix socket endpoint")]
pub struct UnixEndpointError;

impl UnixEndpoint {
    pub fn new(value: &str) -> Result<Self, UnixEndpointError> {
        Endpoint::parse(value).map(Self).map_err(|_| UnixEndpointError)
    }

    pub fn endpoint(&self) -> Endpoint {
        self.0.clone()
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
