/// A string is not a bare registry `host[:port]`.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RegistryHostError {
    #[error("registry is not a valid host and optional port")]
    Malformed,
    #[error("registry must not carry credentials")]
    Credentials,
    #[error("registry must not carry a path, query, or fragment")]
    ExtraComponents,
}
