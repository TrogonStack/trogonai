/// The port the in-guest agent listens on by default.
pub const DEFAULT_AGENT_VSOCK_PORT: u32 = 8110;

/// The smallest CID a guest may take: 0 to 2 are the hypervisor, the reserved value and the host.
pub const MIN_GUEST_CID: u32 = 3;
