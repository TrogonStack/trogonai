use k8s_openapi::api::core::v1::{ConfigMap, Container, Volume};

/// The sidecar container, its supporting init container, the ConfigMap holding the JSON-serialized policy, and the
/// `Volume` mounting that ConfigMap, all needed to enforce domain-based `rules` via the egress proxy sidecar.
pub struct EgressSidecarResources {
    pub sidecar_container: Container,
    pub init_container: Container,
    pub config_map: ConfigMap,
    pub volume: Volume,
    /// Shared `emptyDir` volume the sidecar writes its generated CA into and the `agent` container reads it from.
    /// `None` when the policy has no `Transform` or `Forward` rule, since the sidecar then never terminates TLS
    /// and would never populate the file: callers must not point the `agent` container's TLS clients at a CA
    /// bundle that never gets written, since that breaks every HTTPS connection, not just the ones meant to be
    /// intercepted. When `Some`, callers must add both this volume and a mount for it on the `agent` container
    /// (see `EGRESS_PROXY_CA_VOLUME_NAME` and `EGRESS_PROXY_CA_MOUNT_PATH`).
    pub ca_volume: Option<Volume>,
}
