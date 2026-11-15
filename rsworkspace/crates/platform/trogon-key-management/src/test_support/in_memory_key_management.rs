use super::{TestAdmission, TestKeyState, test_key::TestKey, test_operation::TestOperation, test_store::TestStore};
use crate::{
    AuthenticatedBinding, Dek, KeyOperationError, KeyPurpose, KeyRef, KeyUseDenialReason, MaterialSuite, TenantId,
    UnwrapDek, WrapDek, WrappedDek,
};
use aws_lc_rs::{
    aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey},
    rand::{SecureRandom, SystemRandom},
};
use std::sync::Mutex;
use zeroize::Zeroizing;

// Test-only AEAD fixture. One lock makes local admission/state and crypto atomic;
// it is not a model of service awaits, durable registration or provider custody.
#[derive(Default)]
pub struct InMemoryKeyManagement {
    store: Mutex<TestStore>,
}

impl InMemoryKeyManagement {
    pub fn install_key(
        &self,
        key: KeyRef,
        tenant: TenantId,
        purpose: KeyPurpose,
        material: Zeroizing<[u8; 32]>,
    ) -> Result<(), KeyOperationError> {
        let mut store = self.store.lock().map_err(|_| KeyOperationError::InternalFailure)?;
        if store.keys.contains_key(&key) {
            return Err(KeyOperationError::InternalFailure);
        }
        store.keys.insert(
            key,
            TestKey {
                material,
                tenant,
                purpose,
                state: TestKeyState::Active,
            },
        );
        Ok(())
    }
    pub fn set_admission(&self, admission: TestAdmission) -> Result<(), KeyOperationError> {
        self.store
            .lock()
            .map_err(|_| KeyOperationError::InternalFailure)?
            .admission = admission;
        Ok(())
    }
    pub fn set_key_state(&self, key: &KeyRef, state: TestKeyState) -> Result<(), KeyOperationError> {
        self.store
            .lock()
            .map_err(|_| KeyOperationError::InternalFailure)?
            .keys
            .get_mut(key)
            .ok_or(KeyOperationError::KeyUseDenied(KeyUseDenialReason::ProviderDenied))?
            .state = state;
        Ok(())
    }
    fn with_key<T>(
        &self,
        key: &KeyRef,
        binding: &AuthenticatedBinding,
        operation: TestOperation,
        action: impl FnOnce(&TestKey) -> Result<T, KeyOperationError>,
    ) -> Result<T, KeyOperationError> {
        let store = self.store.lock().map_err(|_| KeyOperationError::InternalFailure)?;
        match store.admission {
            TestAdmission::Enabled => {}
            TestAdmission::Paused => return Err(KeyOperationError::KeyUseDenied(KeyUseDenialReason::BackendPaused)),
            TestAdmission::NotActive => {
                return Err(KeyOperationError::KeyUseDenied(KeyUseDenialReason::BackendNotActive));
            }
        }
        let entry = store
            .keys
            .get(key)
            .ok_or(KeyOperationError::KeyUseDenied(KeyUseDenialReason::ProviderDenied))?;
        if entry.tenant != *binding.tenant() || entry.purpose != *binding.purpose() {
            return Err(KeyOperationError::BindingMismatch);
        }
        if entry.state != TestKeyState::Active
            && !(operation == TestOperation::Unwrap && entry.state == TestKeyState::DecryptOnly)
        {
            return Err(KeyOperationError::KeyUseDenied(KeyUseDenialReason::KeyState));
        }
        action(entry)
    }
}

impl WrapDek for InMemoryKeyManagement {
    type Error = KeyOperationError;
    async fn wrap_dek(
        &self,
        key: &KeyRef,
        dek: Dek,
        binding: &AuthenticatedBinding,
    ) -> Result<WrappedDek, Self::Error> {
        self.with_key(key, binding, TestOperation::Wrap, |entry| {
            let cipher = LessSafeKey::new(
                UnboundKey::new(&AES_256_GCM, entry.material.as_ref())
                    .map_err(|_| KeyOperationError::InternalFailure)?,
            );
            let mut nonce = [0; 12];
            SystemRandom::new()
                .fill(&mut nonce)
                .map_err(|_| KeyOperationError::InternalFailure)?;
            let mut buffer = Zeroizing::new(Vec::with_capacity(32 + AES_256_GCM.tag_len()));
            buffer.extend_from_slice(dek.expose());
            cipher
                .seal_in_place_append_tag(
                    Nonce::assume_unique_for_key(nonce),
                    Aad::from(binding.canonical_bytes()),
                    &mut *buffer,
                )
                .map_err(|_| KeyOperationError::InternalFailure)?;
            let mut payload = nonce.to_vec();
            payload.extend_from_slice(&buffer);
            WrappedDek::new(MaterialSuite::Dek32V1, 1, payload).map_err(|_| KeyOperationError::InternalFailure)
        })
    }
}

impl UnwrapDek for InMemoryKeyManagement {
    type Error = KeyOperationError;
    async fn unwrap_dek(
        &self,
        key: &KeyRef,
        wrapped: &WrappedDek,
        binding: &AuthenticatedBinding,
    ) -> Result<Dek, Self::Error> {
        self.with_key(key, binding, TestOperation::Unwrap, |entry| {
            let payload = wrapped.adapter_payload();
            if payload.len() != 60 {
                return Err(KeyOperationError::IntegrityFailure);
            }
            let cipher = LessSafeKey::new(
                UnboundKey::new(&AES_256_GCM, entry.material.as_ref())
                    .map_err(|_| KeyOperationError::InternalFailure)?,
            );
            let mut nonce = [0; 12];
            nonce.copy_from_slice(&payload[..12]);
            let mut buffer = Zeroizing::new(payload[12..].to_vec());
            let plaintext_len = cipher
                .open_in_place(
                    Nonce::assume_unique_for_key(nonce),
                    Aad::from(binding.canonical_bytes()),
                    &mut buffer,
                )
                .map_err(|_| KeyOperationError::IntegrityFailure)?
                .len();
            buffer.truncate(plaintext_len);
            Dek::new(buffer).map_err(|_| KeyOperationError::IntegrityFailure)
        })
    }
}
