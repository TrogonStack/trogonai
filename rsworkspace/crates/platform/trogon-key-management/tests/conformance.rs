#![cfg(feature = "test-support")]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use trogon_key_management::test_support::{InMemoryKeyManagement, TestAdmission, TestKeyState};
use trogon_key_management::{
    AuthenticatedBinding, BindingVersion, Dek, KeyOperationError, KeyPurpose, KeyRef, KeyUseDenialReason,
    MaterialSuite, ResourceId, ResourceTypeId, TenantId, UnwrapDek, WrapDek, WrappedDek,
};
use zeroize::Zeroizing;

fn binding(tenant: &str, resource_type: &str, resource: &str, purpose: &str) -> AuthenticatedBinding {
    AuthenticatedBinding::new(
        TenantId::new(tenant).unwrap(),
        ResourceTypeId::new(resource_type).unwrap(),
        ResourceId::new(resource).unwrap(),
        KeyPurpose::new(purpose).unwrap(),
        BindingVersion::new(1).unwrap(),
    )
}
fn fixture() -> (InMemoryKeyManagement, KeyRef, KeyRef, AuthenticatedBinding) {
    let fake = InMemoryKeyManagement::default();
    let first = KeyRef::new("key-a").unwrap();
    let second = KeyRef::new("key-b").unwrap();
    let context = binding("tenant-a", "message", "resource-a", "purpose-a");
    fake.install_key(
        first.clone(),
        context.tenant().clone(),
        context.purpose().clone(),
        Zeroizing::new([7; 32]),
    )
    .unwrap();
    fake.install_key(
        second.clone(),
        context.tenant().clone(),
        context.purpose().clone(),
        Zeroizing::new([8; 32]),
    )
    .unwrap();
    (fake, first, second, context)
}

#[tokio::test]
async fn public_roundtrip_has_binding_and_key_integrity() {
    let (fake, key, other_key, context) = fixture();
    let wrapped = fake
        .wrap_dek(&key, Dek::from_bytes(Zeroizing::new([91; 32])), &context)
        .await
        .unwrap();
    assert_eq!(
        fake.unwrap_dek(&key, &wrapped, &context).await.unwrap().expose(),
        &[91; 32]
    );
    assert_eq!(
        fake.unwrap_dek(&other_key, &wrapped, &context).await.unwrap_err(),
        KeyOperationError::IntegrityFailure
    );
    for different in [
        binding("tenant-b", "message", "resource-a", "purpose-a"),
        binding("tenant-a", "message", "resource-a", "purpose-b"),
    ] {
        assert_eq!(
            fake.unwrap_dek(&key, &wrapped, &different).await.unwrap_err(),
            KeyOperationError::BindingMismatch
        );
    }
    for different in [
        binding("tenant-a", "other-type", "resource-a", "purpose-a"),
        binding("tenant-a", "message", "resource-b", "purpose-a"),
    ] {
        assert_eq!(
            fake.unwrap_dek(&key, &wrapped, &different).await.unwrap_err(),
            KeyOperationError::IntegrityFailure
        );
    }
    let mut tampered = wrapped.adapter_payload().to_vec();
    tampered[12] ^= 1;
    let changed = WrappedDek::new(MaterialSuite::Dek32V1, 1, tampered).unwrap();
    assert_eq!(
        fake.unwrap_dek(&key, &changed, &context).await.unwrap_err(),
        KeyOperationError::IntegrityFailure
    );
}

#[tokio::test]
async fn fake_key_state_and_admission_refuse_before_crypto_use() {
    let (fake, key, _, context) = fixture();
    let wrapped = fake
        .wrap_dek(&key, Dek::from_bytes(Zeroizing::new([91; 32])), &context)
        .await
        .unwrap();
    fake.set_key_state(&key, TestKeyState::DecryptOnly).unwrap();
    assert_eq!(
        fake.wrap_dek(&key, Dek::from_bytes(Zeroizing::new([91; 32])), &context)
            .await
            .unwrap_err(),
        KeyOperationError::KeyUseDenied(KeyUseDenialReason::KeyState)
    );
    assert_eq!(
        fake.unwrap_dek(&key, &wrapped, &context).await.unwrap().expose(),
        &[91; 32]
    );
    for state in [
        TestKeyState::Disabled,
        TestKeyState::PendingDestruction,
        TestKeyState::Destroyed,
    ] {
        fake.set_key_state(&key, state).unwrap();
        assert_eq!(
            fake.unwrap_dek(&key, &wrapped, &context).await.unwrap_err(),
            KeyOperationError::KeyUseDenied(KeyUseDenialReason::KeyState)
        );
    }
    fake.set_key_state(&key, TestKeyState::Active).unwrap();
    for (admission, expected) in [
        (TestAdmission::Paused, KeyUseDenialReason::BackendPaused),
        (TestAdmission::NotActive, KeyUseDenialReason::BackendNotActive),
    ] {
        fake.set_admission(admission).unwrap();
        assert_eq!(
            fake.wrap_dek(&key, Dek::from_bytes(Zeroizing::new([91; 32])), &context)
                .await
                .unwrap_err(),
            KeyOperationError::KeyUseDenied(expected)
        );
        assert_eq!(
            fake.unwrap_dek(&key, &wrapped, &context).await.unwrap_err(),
            KeyOperationError::KeyUseDenied(expected)
        );
    }
}

#[tokio::test]
async fn shared_provider_contract_harness_checks_distinct_binding_dimensions() {
    let (fake, key, other_key, context) = fixture();
    for other in [
        binding("tenant-b", "message", "resource-a", "purpose-a"),
        binding("tenant-a", "message", "resource-a", "purpose-b"),
        binding("tenant-a", "other-type", "resource-a", "purpose-a"),
        binding("tenant-a", "message", "resource-b", "purpose-a"),
    ] {
        trogon_key_management::test_support::check_wrapping_conformance(
            &fake,
            &key,
            &other_key,
            &context,
            &other,
            |error| *error,
        )
        .await
        .unwrap();
    }
}

#[tokio::test]
async fn duplicate_install_cannot_retarget_key_material() {
    let (fake, key, _, context) = fixture();
    let wrapped = fake
        .wrap_dek(&key, Dek::from_bytes(Zeroizing::new([91; 32])), &context)
        .await
        .unwrap();
    assert_eq!(
        fake.install_key(
            key.clone(),
            context.tenant().clone(),
            context.purpose().clone(),
            Zeroizing::new([99; 32])
        )
        .unwrap_err(),
        KeyOperationError::InternalFailure
    );
    assert_eq!(
        fake.unwrap_dek(&key, &wrapped, &context).await.unwrap().expose(),
        &[91; 32]
    );
}

#[tokio::test]
async fn altered_nonce_and_truncated_payload_never_return_material() {
    let (fake, key, _, context) = fixture();
    let wrapped = fake
        .wrap_dek(&key, Dek::from_bytes(Zeroizing::new([91; 32])), &context)
        .await
        .unwrap();
    let mut nonce = wrapped.adapter_payload().to_vec();
    nonce[0] ^= 1;
    for bytes in [nonce, wrapped.adapter_payload()[..59].to_vec(), vec![1]] {
        let changed = WrappedDek::new(MaterialSuite::Dek32V1, 1, bytes).unwrap();
        assert_eq!(
            fake.unwrap_dek(&key, &changed, &context).await.unwrap_err(),
            KeyOperationError::IntegrityFailure
        );
    }
}

struct UnavailableOnDifferentBinding {
    fake: InMemoryKeyManagement,
    expected: AuthenticatedBinding,
}
impl WrapDek for UnavailableOnDifferentBinding {
    type Error = KeyOperationError;
    async fn wrap_dek(
        &self,
        key: &KeyRef,
        dek: Dek,
        binding: &AuthenticatedBinding,
    ) -> Result<WrappedDek, Self::Error> {
        self.fake.wrap_dek(key, dek, binding).await
    }
}
impl UnwrapDek for UnavailableOnDifferentBinding {
    type Error = KeyOperationError;
    async fn unwrap_dek(
        &self,
        key: &KeyRef,
        wrapped: &WrappedDek,
        binding: &AuthenticatedBinding,
    ) -> Result<Dek, Self::Error> {
        if *binding != self.expected {
            return Err(KeyOperationError::ProviderUnavailable);
        }
        self.fake.unwrap_dek(key, wrapped, binding).await
    }
}

#[tokio::test]
async fn conformance_harness_rejects_outage_instead_of_counting_it_as_refusal() {
    let (fake, key, other_key, context) = fixture();
    let other = binding("tenant-a", "message", "resource-b", "purpose-a");
    let faulty = UnavailableOnDifferentBinding {
        fake,
        expected: context.clone(),
    };
    let result = trogon_key_management::test_support::check_wrapping_conformance(
        &faulty,
        &key,
        &other_key,
        &context,
        &other,
        |error| *error,
    )
    .await;
    assert!(matches!(
        result,
        Err(
            trogon_key_management::test_support::ConformanceError::UnexpectedRefusal {
                case: trogon_key_management::test_support::ConformanceCase::BindingSubstitution,
                failure: KeyOperationError::ProviderUnavailable,
                ..
            }
        )
    ));
}

#[tokio::test]
async fn conformance_harness_requires_a_usable_distinct_other_key() {
    let (fake, key, _, context) = fixture();
    let other = binding("tenant-a", "message", "resource-b", "purpose-a");
    let missing = KeyRef::new("not-installed").unwrap();
    let result = trogon_key_management::test_support::check_wrapping_conformance(
        &fake,
        &key,
        &missing,
        &context,
        &other,
        |error| *error,
    )
    .await;
    assert!(matches!(
        result,
        Err(trogon_key_management::test_support::ConformanceError::Wrap(
            KeyOperationError::KeyUseDenied(KeyUseDenialReason::ProviderDenied)
        ))
    ));
}
