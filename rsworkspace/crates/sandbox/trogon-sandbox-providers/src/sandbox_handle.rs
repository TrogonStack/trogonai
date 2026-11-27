use std::collections::HashMap;
use std::path::Path;

use crate::{SandboxError, SandboxStatus};

/// A launched sandbox.
#[async_trait::async_trait]
pub trait SandboxHandle: Send + Sync {
    fn id(&self) -> &str;
    async fn status(&self) -> SandboxStatus;
    async fn cancel(&self) -> Result<(), SandboxError>;

    /// Host path of the egress sidecar's live policy file, if this sandbox has
    /// one (only `UserDefined` network policies launch a sidecar). Callers use
    /// this with `push_network_policy` to update a running sandbox's policy
    /// without restarting it.
    fn policy_file_path(&self) -> Option<&Path> {
        None
    }

    /// Container id of the egress sidecar, if this sandbox has one. The
    /// sidecar outlives a kill of the sandbox container, so a caller that
    /// tears the sandbox down by id must remove this as well.
    fn sidecar_id(&self) -> Option<&str> {
        None
    }

    /// Name of the Kubernetes NetworkPolicy object backing this sandbox, if
    /// one was created at launch time. Callers use this with
    /// `push_network_policy` to update a running sandbox's policy without
    /// restarting it.
    fn network_policy_name(&self) -> Option<&str> {
        None
    }

    /// Name of the Kubernetes ConfigMap holding the egress sidecar's policy
    /// JSON, if one was created at launch time. Callers use this with
    /// `push_network_policy` to update a running sandbox's policy without
    /// restarting it.
    fn config_map_name(&self) -> Option<&str> {
        None
    }

    /// Maps a container-side port from `SandboxSpec::exposed_ports` to a
    /// reachable `host:port` endpoint. Empty for handles that do not publish
    /// ports at launch time.
    fn port_endpoints(&self) -> HashMap<u16, String> {
        HashMap::new()
    }
}
