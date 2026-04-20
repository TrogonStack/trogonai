use std::fmt;

#[derive(Debug)]
pub enum RegistryError {
    /// Failed to create or open the `AGENT_REGISTRY` KV bucket.
    Provision(String),
    /// Failed to write a capability record to KV.
    Put(Box<dyn std::error::Error + Send + Sync>),
    /// Failed to read a capability record from KV.
    Get(Box<dyn std::error::Error + Send + Sync>),
    /// Failed to delete a capability record from KV.
    Delete(Box<dyn std::error::Error + Send + Sync>),
    /// Failed to list all keys from KV.
    List(Box<dyn std::error::Error + Send + Sync>),
    /// Failed to serialize an `AgentCapability` to JSON.
    Serialization(serde_json::Error),
    /// Failed to deserialize a raw KV value into an `AgentCapability`.
    Deserialization(serde_json::Error),
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegistryError::Provision(msg) => write!(f, "registry provision error: {msg}"),
            RegistryError::Put(e) => write!(f, "registry put error: {e}"),
            RegistryError::Get(e) => write!(f, "registry get error: {e}"),
            RegistryError::Delete(e) => write!(f, "registry delete error: {e}"),
            RegistryError::List(e) => write!(f, "registry list error: {e}"),
            RegistryError::Serialization(e) => write!(f, "registry serialization error: {e}"),
            RegistryError::Deserialization(e) => {
                write!(f, "registry deserialization error: {e}")
            }
        }
    }
}

impl std::error::Error for RegistryError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            RegistryError::Put(e) => Some(e.as_ref()),
            RegistryError::Get(e) => Some(e.as_ref()),
            RegistryError::Delete(e) => Some(e.as_ref()),
            RegistryError::List(e) => Some(e.as_ref()),
            RegistryError::Serialization(e) => Some(e),
            RegistryError::Deserialization(e) => Some(e),
            RegistryError::Provision(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json_err() -> serde_json::Error {
        serde_json::from_str::<u32>("{bad}").unwrap_err()
    }

    fn boxed(msg: &'static str) -> Box<dyn std::error::Error + Send + Sync> {
        Box::new(std::io::Error::other(msg))
    }

    #[test]
    fn display_provision() {
        assert!(format!("{}", RegistryError::Provision("x".into())).contains("provision error"));
    }

    #[test]
    fn display_put() {
        assert!(format!("{}", RegistryError::Put(boxed("x"))).contains("put error"));
    }

    #[test]
    fn display_get() {
        assert!(format!("{}", RegistryError::Get(boxed("x"))).contains("get error"));
    }

    #[test]
    fn display_delete() {
        assert!(format!("{}", RegistryError::Delete(boxed("x"))).contains("delete error"));
    }

    #[test]
    fn display_list() {
        assert!(format!("{}", RegistryError::List(boxed("x"))).contains("list error"));
    }

    #[test]
    fn display_serialization() {
        assert!(
            format!("{}", RegistryError::Serialization(json_err())).contains("serialization error")
        );
    }

    #[test]
    fn display_deserialization() {
        assert!(
            format!("{}", RegistryError::Deserialization(json_err()))
                .contains("deserialization error")
        );
    }
}
