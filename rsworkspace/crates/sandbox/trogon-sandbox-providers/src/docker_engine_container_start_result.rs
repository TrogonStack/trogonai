use std::collections::HashMap;

/// Return value of [`crate::DockerEngine::create_and_start`]: the started container's
/// id plus a container-port to `host:port` map resolved from
/// [`crate::ContainerRunSpec::exposed_ports`]. `port_endpoints` omits any requested
/// port Docker did not bind, rather than erroring the whole launch.
#[derive(Debug, Clone)]
pub struct ContainerStartResult {
    pub container_id: String,
    pub port_endpoints: HashMap<u16, String>,
}
