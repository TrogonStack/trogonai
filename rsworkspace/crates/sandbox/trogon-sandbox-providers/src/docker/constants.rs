/// Label naming the worker that owns a container.
pub const DOCKER_LABEL_OWNER: &str = "sandbox.trogon.live/owner";
/// Label naming the lease a container serves. Absent on warm-pool containers.
pub const DOCKER_LABEL_LEASE_ID: &str = "sandbox.trogon.live/lease-id";
/// Label naming what a container is for: `workload`, `egress-sidecar`, `warm` or `restored`.
pub const DOCKER_LABEL_ROLE: &str = "sandbox.trogon.live/role";

/// Mount point inside the sandbox container for the tenant's workspace,
/// whether it arrives via git clone (`spec.clone`) or a host bind mount
/// (`spec.host_mount`).
pub const WORKSPACE_MOUNT_PATH: &str = "/workspace";

/// Capabilities the egress sidecar keeps. `NET_ADMIN` installs the redirect rules; the other
/// three are what `setpriv` needs to drop to the proxy uid, clear its groups and empty its
/// bounding set before the proxy starts. Everything else is dropped.
pub const SIDECAR_CAPABILITIES: [&str; 4] = ["NET_ADMIN", "SETUID", "SETGID", "SETPCAP"];

/// Scratch mounts for the read-only sidecar: `/run` holds the `iptables` lock file and `/tmp`
/// is the proxy's temp space. Both are small, non-executable and not setuid.
pub const SIDECAR_TMPFS_OPTIONS: &str = "rw,noexec,nosuid,nodev,size=8m";

/// Process ceiling for the egress sidecar, which runs a shell, `iptables` and one proxy.
pub const SIDECAR_PIDS_LIMIT: i64 = 256;

/// Where the egress CA bundle appears inside the sandbox container.
pub const SANDBOX_CA_PATH: &str = "/etc/sandbox-egress-proxy-ca/ca.pem";

/// Where the policy file appears inside the egress sidecar.
pub const SIDECAR_POLICY_PATH: &str = "/etc/sandbox-egress-proxy/policy.json";

/// Address the egress proxy listens on inside the shared network namespace.
pub const SIDECAR_LISTEN_ADDR: &str = "127.0.0.1:3128";

/// Port part of [`SIDECAR_LISTEN_ADDR`].
pub const SIDECAR_LISTEN_PORT: &str = "3128";

/// Host name the sidecar resolves to reach a loopback NATS server on the Docker host.
pub const DOCKER_HOST_ALIAS: &str = "host.docker.internal";

/// Docker Engine API version used for requests bollard does not model.
pub const ENGINE_API_VERSION: &str = "v1.43";

/// Socket the Docker daemon listens on unless told otherwise.
pub const DEFAULT_DOCKER_SOCKET: &str = "/var/run/docker.sock";

/// Environment names the egress proxy daemon reads for its listen address, policy file and CA output.
pub const PROXY_LISTEN_ADDR_ENV: &str = "EGRESS_PROXY_LISTEN_ADDR";
pub const PROXY_POLICY_FILE_ENV: &str = "EGRESS_PROXY_POLICY_FILE";
pub const PROXY_CA_OUT_FILE_ENV: &str = "EGRESS_PROXY_CA_OUT_FILE";

/// Where the CA bundle appears inside the egress sidecar (writable there, read-only in the sandbox).
pub const SIDECAR_CA_PATH: &str = "/etc/sandbox-egress-proxy-ca/ca.pem";

/// Executable the egress sidecar runs after dropping privileges.
pub const PROXY_BINARY: &str = "sandbox-egress-proxyd";

/// Seconds bollard waits on one request to the daemon.
pub const REQUEST_TIMEOUT_SECS: u64 = 120;
