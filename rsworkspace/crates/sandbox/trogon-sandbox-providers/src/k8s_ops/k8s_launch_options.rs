/// Per-lease metadata and optional pre-staged workspace tar secrets for init containers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct K8sLaunchOptions {
    pub lease_id: Option<String>,
    pub tenant_id: Option<String>,
    pub digest_label: Option<String>,
    pub restore_secret_name: Option<String>,
    pub s3_volume_secret_name: Option<String>,
    /// Runs a shell keep-alive loop in the agent container instead of the
    /// image's entrypoint, so the pod stays up for exec whatever the image
    /// is. The image must ship `/bin/sh`.
    pub keep_alive: bool,
}
