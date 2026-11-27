use crate::OrdinaryCommandRuntimeError;
use crate::guest_architecture::GuestArchitecture;
use crate::guest_platform_wire::GuestPlatformWire;

/// A sandbox that can run the bridge: Linux with Node 22.7 or newer, on a known architecture.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) struct GuestPlatform {
    pub(crate) architecture: GuestArchitecture,
}

impl GuestPlatform {
    /// Reads the probe output; a sandbox that cannot run the bridge is refused.
    pub(crate) fn parse(output: &str) -> Result<Self, OrdinaryCommandRuntimeError> {
        let wire: GuestPlatformWire =
            serde_json::from_str(output).map_err(|_| OrdinaryCommandRuntimeError::GuestIdentityMalformed)?;
        let new_enough = wire.node_major > 22 || (wire.node_major == 22 && wire.node_minor >= 7);
        if wire.platform != "linux" || !new_enough {
            return Err(OrdinaryCommandRuntimeError::GuestUnsupported);
        }
        Ok(Self {
            architecture: GuestArchitecture::from_node(&wire.architecture)?,
        })
    }
}

#[cfg(test)]
mod tests;
