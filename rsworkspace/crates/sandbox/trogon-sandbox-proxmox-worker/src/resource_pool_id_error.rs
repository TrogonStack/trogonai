/// A resource pool must be a nonempty, non-nested Proxmox pool id.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("resource pool must be a nonempty, non-nested Proxmox pool id")]
pub struct ResourcePoolIdError;
