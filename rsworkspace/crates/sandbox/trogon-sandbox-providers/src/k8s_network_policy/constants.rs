/// Placeholder image reference for the egress-proxy sidecar. Replace it once the proxy image is published.
pub const EGRESS_PROXY_IMAGE: &str = "sandbox-egress-proxy:latest";

pub(super) const EGRESS_PROXY_LISTEN_PORT: &str = "3128";
pub(crate) const EGRESS_PROXY_CONFIGMAP_KEY: &str = "policy.json";
pub(super) const EGRESS_PROXY_POLICY_MOUNT_PATH: &str = "/etc/sandbox-egress-proxy";
pub(super) const EGRESS_PROXY_POLICY_VOLUME_NAME: &str = "egress-proxy-policy";

/// Shared `emptyDir` volume name carrying the sidecar-generated CA cert between the sidecar and the `agent`
/// container in the same Pod, so the sandboxed workload can trust TLS terminated by a `Transform` or `Forward`
/// domain rule instead of seeing every such connection as a MITM.
pub const EGRESS_PROXY_CA_VOLUME_NAME: &str = "egress-proxy-ca";
pub const EGRESS_PROXY_CA_MOUNT_PATH: &str = "/etc/sandbox-egress-proxy-ca";
pub(super) const EGRESS_PROXY_CA_FILE: &str = "ca.pem";
