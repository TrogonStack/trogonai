#![cfg(feature = "test-support")]
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use trogon_key_management::test_support::{
    ConformanceCase, ConformanceError, InMemoryKeyManagement, TestAdmission, check_wrapping_conformance,
};
use trogon_key_management::{
    AuthenticatedBinding, BindingVersion, Dek, KeyOperationError, KeyPurpose, KeyRef, KeyUseDenialReason, ResourceId,
    ResourceTypeId, TenantId, UnwrapDek, WrapDek, WrappedDek,
};
use zeroize::Zeroizing;

#[derive(Clone, Copy)]
enum Fault {
    None,
    AcceptRejected(ConformanceCase),
    ReplaceRefusal(ConformanceCase, KeyOperationError),
    ChangePlaintext,
    RefuseValidUnwrap,
}

struct FaultyAdapter {
    reference: InMemoryKeyManagement,
    primary_key: KeyRef,
    original_binding: AuthenticatedBinding,
    fault: Fault,
}

impl WrapDek for FaultyAdapter {
    type Error = KeyOperationError;

    async fn wrap_dek(
        &self,
        key: &KeyRef,
        dek: Dek,
        binding: &AuthenticatedBinding,
    ) -> Result<WrappedDek, Self::Error> {
        self.reference.wrap_dek(key, dek, binding).await
    }
}

impl UnwrapDek for FaultyAdapter {
    type Error = KeyOperationError;

    async fn unwrap_dek(
        &self,
        key: &KeyRef,
        wrapped: &WrappedDek,
        binding: &AuthenticatedBinding,
    ) -> Result<Dek, Self::Error> {
        match self.reference.unwrap_dek(key, wrapped, binding).await {
            Ok(dek) => match self.fault {
                Fault::ChangePlaintext => Ok(Dek::from_bytes(Zeroizing::new([0x19; 32]))),
                Fault::RefuseValidUnwrap => Err(KeyOperationError::ProviderUnavailable),
                _ => Ok(dek),
            },
            Err(source) => {
                let case = if *key != self.primary_key {
                    ConformanceCase::KeySubstitution
                } else if *binding != self.original_binding {
                    ConformanceCase::BindingSubstitution
                } else {
                    ConformanceCase::Tampering
                };
                match self.fault {
                    Fault::AcceptRejected(selected) if selected == case => {
                        Ok(Dek::from_bytes(Zeroizing::new([0x19; 32])))
                    }
                    Fault::ReplaceRefusal(selected, failure) if selected == case => Err(failure),
                    _ => Err(source),
                }
            }
        }
    }
}

struct Fixture {
    adapter: FaultyAdapter,
    key: KeyRef,
    other_key: KeyRef,
    binding: AuthenticatedBinding,
    other_binding: AuthenticatedBinding,
}

impl Fixture {
    fn new(fault: Fault) -> Self {
        let reference = InMemoryKeyManagement::default();
        let key = KeyRef::new("primary").unwrap();
        let other_key = KeyRef::new("alternate").unwrap();
        let binding = AuthenticatedBinding::new(
            TenantId::new("tenant").unwrap(),
            ResourceTypeId::new("document").unwrap(),
            ResourceId::new("resource").unwrap(),
            KeyPurpose::new("purpose").unwrap(),
            BindingVersion::new(1).unwrap(),
        );
        let other_binding = AuthenticatedBinding::new(
            binding.tenant().clone(),
            binding.resource_type().clone(),
            ResourceId::new("other-resource").unwrap(),
            binding.purpose().clone(),
            binding.version(),
        );
        for (key_ref, material) in [(&key, [7; 32]), (&other_key, [8; 32])] {
            reference
                .install_key(
                    key_ref.clone(),
                    binding.tenant().clone(),
                    binding.purpose().clone(),
                    Zeroizing::new(material),
                )
                .unwrap();
        }
        Self {
            adapter: FaultyAdapter {
                reference,
                primary_key: key.clone(),
                original_binding: binding.clone(),
                fault,
            },
            key,
            other_key,
            binding,
            other_binding,
        }
    }

    async fn check(&self) -> Result<(), ConformanceError<KeyOperationError>> {
        check_wrapping_conformance(
            &self.adapter,
            &self.key,
            &self.other_key,
            &self.binding,
            &self.other_binding,
            |error| *error,
        )
        .await
    }
}

#[tokio::test]
async fn correct_adapter_still_satisfies_all_controls() {
    Fixture::new(Fault::None).check().await.unwrap();
}

#[tokio::test]
async fn aliased_fixtures_are_rejected_before_backend_admission() {
    let fixture = Fixture::new(Fault::None);
    fixture.adapter.reference.set_admission(TestAdmission::Paused).unwrap();
    for (other_key, other_binding) in [
        (&fixture.key, &fixture.other_binding),
        (&fixture.other_key, &fixture.binding),
    ] {
        assert!(matches!(
            check_wrapping_conformance(
                &fixture.adapter,
                &fixture.key,
                other_key,
                &fixture.binding,
                other_binding,
                |error| *error,
            )
            .await,
            Err(ConformanceError::AliasedFixture)
        ));
    }
}

#[tokio::test]
async fn accepted_substitutions_and_tampering_fail_conformance() {
    for case in [
        ConformanceCase::KeySubstitution,
        ConformanceCase::BindingSubstitution,
        ConformanceCase::Tampering,
    ] {
        let result = Fixture::new(Fault::AcceptRejected(case)).check().await;
        match (case, result) {
            (ConformanceCase::KeySubstitution, Err(ConformanceError::KeySubstitutionAccepted))
            | (ConformanceCase::BindingSubstitution, Err(ConformanceError::BindingSubstitutionAccepted))
            | (ConformanceCase::Tampering, Err(ConformanceError::TamperingAccepted)) => {}
            (case, result) => panic!("{case:?} was not rejected as accepted evidence: {result:?}"),
        }
    }
}

#[tokio::test]
async fn operational_and_authorization_failures_are_not_negative_proofs() {
    for case in [
        ConformanceCase::KeySubstitution,
        ConformanceCase::BindingSubstitution,
        ConformanceCase::Tampering,
    ] {
        for refusal in [
            KeyOperationError::ProviderUnavailable,
            KeyOperationError::InternalFailure,
            KeyOperationError::KeyUseDenied(KeyUseDenialReason::ProviderDenied),
        ] {
            let result = Fixture::new(Fault::ReplaceRefusal(case, refusal)).check().await;
            assert!(matches!(
                result,
                Err(ConformanceError::UnexpectedRefusal {
                    case: reported_case,
                    failure,
                    source,
                }) if reported_case == case && failure == refusal && source == refusal
            ));
        }
    }
}

#[tokio::test]
async fn tampering_requires_integrity_failure_even_when_binding_mismatch_is_cryptographic() {
    assert!(matches!(
        Fixture::new(Fault::ReplaceRefusal(
            ConformanceCase::Tampering,
            KeyOperationError::BindingMismatch,
        ))
        .check()
        .await,
        Err(ConformanceError::UnexpectedRefusal {
            case: ConformanceCase::Tampering,
            failure: KeyOperationError::BindingMismatch,
            source: KeyOperationError::BindingMismatch,
        })
    ));
    Fixture::new(Fault::ReplaceRefusal(
        ConformanceCase::Tampering,
        KeyOperationError::IntegrityFailure,
    ))
    .check()
    .await
    .unwrap();
}

#[tokio::test]
async fn successful_round_trip_must_preserve_plaintext() {
    assert!(matches!(
        Fixture::new(Fault::ChangePlaintext).check().await,
        Err(ConformanceError::PlaintextMismatch)
    ));
}

#[tokio::test]
async fn positive_unwrap_failures_preserve_native_error() {
    assert!(matches!(
        Fixture::new(Fault::RefuseValidUnwrap).check().await,
        Err(ConformanceError::Unwrap(KeyOperationError::ProviderUnavailable))
    ));
}
