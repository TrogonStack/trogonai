#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use trogon_key_management::{
    AuthenticatedBinding, BindingVersion, Dek, KeyPurpose, KeyRef, MaterialSuite, ResourceId, ResourceTypeId, TenantId,
    WrappedDek,
};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

fn binding(tenant: &str, resource_type: &str, resource: &str, purpose: &str) -> AuthenticatedBinding {
    AuthenticatedBinding::new(
        TenantId::new(tenant).unwrap(),
        ResourceTypeId::new(resource_type).unwrap(),
        ResourceId::new(resource).unwrap(),
        KeyPurpose::new(purpose).unwrap(),
        BindingVersion::new(1).unwrap(),
    )
}

#[test]
fn exact_binding_vector_matches_the_current_contract() {
    let mut expected = vec![0, 0, 0, 30];
    expected.extend_from_slice(b"trogon/key-wrapping/binding/v1");
    expected.extend_from_slice(&[
        0, 0, 0, 1, 0, 0, 0, 1, b't', 0, 0, 0, 1, b'k', 0, 0, 0, 1, b'r', 0, 0, 0, 1, b'p',
    ]);
    assert_eq!(binding("t", "k", "r", "p").canonical_bytes(), expected);
}

#[test]
fn binding_can_be_reconstructed_from_its_public_components() {
    let original = binding("tenant", "document", "resource", "purpose");
    assert_eq!(original.tenant().as_str(), "tenant");
    assert_eq!(original.resource_type().as_str(), "document");
    assert_eq!(original.resource().as_str(), "resource");
    assert_eq!(original.purpose().as_str(), "purpose");
    assert_eq!(original.version(), BindingVersion::new(1).unwrap());
    let reconstructed = AuthenticatedBinding::new(
        original.tenant().clone(),
        original.resource_type().clone(),
        original.resource().clone(),
        original.purpose().clone(),
        original.version(),
    );
    assert_eq!(reconstructed, original);
    assert_eq!(reconstructed.canonical_bytes(), original.canonical_bytes());
}

#[test]
fn binding_encoding_separates_resource_type_and_field_boundaries() {
    assert_ne!(
        binding("a", "bc", "r", "p").canonical_bytes(),
        binding("ab", "c", "r", "p").canonical_bytes()
    );
    assert_ne!(
        binding("t", "type-a", "r", "p").canonical_bytes(),
        binding("t", "type-b", "r", "p").canonical_bytes()
    );
}

#[test]
fn identity_limits_count_characters_but_binding_lengths_count_utf8_bytes() {
    let maximum = "é".repeat(256);
    let overlong = "é".repeat(257);
    assert!(TenantId::new(maximum.clone()).is_ok());
    assert!(ResourceTypeId::new(maximum.clone()).is_ok());
    assert!(ResourceId::new(maximum.clone()).is_ok());
    assert!(KeyPurpose::new(maximum.clone()).is_ok());
    assert!(TenantId::new(overlong.clone()).is_err());
    assert!(ResourceTypeId::new(overlong.clone()).is_err());
    assert!(ResourceId::new(overlong.clone()).is_err());
    assert!(KeyPurpose::new(overlong).is_err());
    let value = binding(&maximum, "k", "r", "p").canonical_bytes();
    assert_eq!(&value[38..42], &[0, 0, 2, 0]);
    assert_eq!(&value[42..554], maximum.as_bytes());
}

#[test]
fn versions_suites_and_payloads_are_closed() {
    assert!(BindingVersion::new(0).is_err());
    assert!(BindingVersion::new(2).is_err());
    assert_eq!(MaterialSuite::Dek32V1.get(), 1);
    assert_eq!(MaterialSuite::try_from(1).unwrap(), MaterialSuite::Dek32V1);
    assert_eq!(BindingVersion::new(1).unwrap().get(), 1);
    assert!(MaterialSuite::try_from(0).is_err());
    assert!(MaterialSuite::try_from(2).is_err());
    assert!(WrappedDek::new(MaterialSuite::Dek32V1, 0, vec![1]).is_err());
    assert!(WrappedDek::new(MaterialSuite::Dek32V1, 2, vec![1]).is_err());
    assert!(WrappedDek::new(MaterialSuite::Dek32V1, 1, vec![]).is_err());
    assert!(WrappedDek::new(MaterialSuite::Dek32V1, 1, vec![1; 65537]).is_err());
    let wrapped = WrappedDek::new(MaterialSuite::Dek32V1, 1, vec![1; 65536]).unwrap();
    assert_eq!(wrapped.adapter_payload().len(), 65536);
    assert_eq!(wrapped.format_version(), 1);
    assert_eq!(wrapped.suite(), MaterialSuite::Dek32V1);
}

#[test]
fn identifiers_retain_exact_input_and_key_refs_keep_existing_nonempty_contract() {
    assert!(TenantId::new("").is_err());
    assert!(ResourceTypeId::new("").is_err());
    assert!(ResourceId::new("").is_err());
    assert!(KeyPurpose::new("").is_err());
    assert!(KeyRef::new("").is_err());
    assert_eq!(KeyRef::new("x".repeat(4096)).unwrap().as_str().len(), 4096);
    let a = binding("tenant", "type", "é", "purpose");
    let b = binding("tenant", "type", "e\u{301}", "purpose");
    assert_ne!(a.canonical_bytes(), b.canonical_bytes());
}

#[test]
fn dek_is_owned_redacted_and_zeroizable_on_all_owned_paths() {
    fn require_drop_zeroization<T: ZeroizeOnDrop>() {}
    require_drop_zeroization::<Dek>();
    assert!(Dek::new(Zeroizing::new(vec![71; 31])).is_err());
    assert!(Dek::new(Zeroizing::new(vec![71; 33])).is_err());
    let mut dek = Dek::new(Zeroizing::new(vec![71; 32])).unwrap();
    assert_eq!(format!("{dek:?}"), "Dek([REDACTED])");
    assert_eq!(dek.expose(), &[71; 32]);
    dek.zeroize();
    assert_eq!(dek.expose(), &[0; 32]);
}

#[test]
fn wrapped_debug_retains_only_public_shape() {
    let wrapped = WrappedDek::new(MaterialSuite::Dek32V1, 1, b"ciphertext-marker".to_vec()).unwrap();
    let debug = format!("{wrapped:?}");
    assert!(debug.contains("payload_len: 17"));
    assert!(!debug.contains("99, 105, 112"));
    assert!(!debug.contains("ciphertext-marker"));
}
