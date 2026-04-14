use std::fmt;

#[derive(Debug)]
pub enum RegistryError {
    /// Failed to create or open the `AGENT_REGISTRY` KV bucket.
    Provision(String),
    /// Failed to write a capability record to KV.
    Put(String),
    /// Failed to read a capability record from KV.
    Get(String),
    /// Failed to delete a capability record from KV.
    Delete(String),
    /// Failed to list all keys from KV.
    List(String),
    /// Failed to serialize an `AgentCapability` to JSON.
    Serialization(serde_json::Error),
    /// Failed to deserialize a raw KV value into an `AgentCapability`.
    Deserialization(serde_json::Error),
}

impl fmt::Display for RegistryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RegistryError::Provision(msg) => write!(f, "registry provision error: {msg}"),
            RegistryError::Put(msg) => write!(f, "registry put error: {msg}"),
            RegistryError::Get(msg) => write!(f, "registry get error: {msg}"),
            RegistryError::Delete(msg) => write!(f, "registry delete error: {msg}"),
            RegistryError::List(msg) => write!(f, "registry list error: {msg}"),
            RegistryError::Serialization(e) => write!(f, "registry serialization error: {e}"),
            RegistryError::Deserialization(e) => {
                write!(f, "registry deserialization error: {e}")
            }
        }
    }
}

impl std::error::Error for RegistryError {}
