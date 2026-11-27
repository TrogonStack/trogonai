/// Default ingress port. The runtime publishes it as the container's readiness port, so changing it means rebuilding
/// every image that names it.
pub(crate) const DEFAULT_AGENT_PORT: u16 = 8110;
