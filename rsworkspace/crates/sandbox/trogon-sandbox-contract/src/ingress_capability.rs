use serde::{Deserialize, Serialize};

/// How many ports a worker can publish for one sandbox.
#[derive(Debug, Clone, Copy, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct IngressCapability {
    /// `None` is unknown or unlimited; `Some(0)` means the worker publishes no ports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_exposed_ports: Option<u16>,
}

impl IngressCapability {
    pub fn allows(&self, count: u16) -> bool {
        self.max_exposed_ports.is_none_or(|max| count <= max)
    }
}
