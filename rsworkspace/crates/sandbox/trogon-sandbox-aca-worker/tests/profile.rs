//! What the worker advertises: no published ports and no mounts, because Dynamic Sessions offers neither.

use trogon_sandbox_aca_worker::PROFILE;
use trogon_sandbox_contract::CapabilitySupport;

#[test]
fn no_port_is_published_and_preview_is_not_advertised() {
    assert_eq!(PROFILE.ingress.max_exposed_ports, Some(0));
    assert_eq!(PROFILE.isolation.supports_port_preview, CapabilitySupport::Unsupported);
    assert_eq!(
        PROFILE.isolation.supports_expose_after_create,
        CapabilitySupport::Unsupported
    );
}

#[test]
fn no_fuse_mount_is_advertised() {
    assert_eq!(PROFILE.volume.supports_fuse_mounts, CapabilitySupport::Unsupported);
}
