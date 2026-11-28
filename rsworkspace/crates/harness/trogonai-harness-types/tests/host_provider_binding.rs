//! Host provider binding and saved authority contract.

use trogonai_harness_types::{ContentHash, CredentialAuthority, CredentialSource, HostProviderBinding};

#[test]
fn explicit_host_binding_has_a_new_fixed_domain_and_never_the_generic_slot_identity() {
    let binding = HostProviderBinding::new(ContentHash::of(b"host-admitted-service-authority-a"));
    assert_eq!(binding.digest().as_bytes().len(), 32);
    let identity = CredentialAuthority::derive_host_provider(binding);
    assert_eq!(
        identity.digest().to_string(),
        "cb2b33c161ec034f11559066fbe4643904d2e8272a46f30945d271f87e6f9c8d"
    );
    assert_ne!(
        Some(identity),
        CredentialAuthority::derive(CredentialSource::HostManaged, None)
    );
    assert_ne!(
        identity,
        CredentialAuthority::derive_host_provider(HostProviderBinding::new(ContentHash::of(
            b"host-admitted-service-authority-b"
        )))
    );
}
