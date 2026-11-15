#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

use std::{
    future::{Future, pending},
    pin::pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll, Waker},
};
use trogon_key_management::{
    AuthenticatedBinding, BindingVersion, Dek, KeyOperationError, KeyPurpose, KeyRef, ResourceId, ResourceTypeId,
    TenantId, WrapDek, WrappedDek,
};
use zeroize::{Zeroize, Zeroizing};

struct RetainedMaterial {
    material: Dek,
    cleared: Arc<AtomicBool>,
}
impl Drop for RetainedMaterial {
    fn drop(&mut self) {
        self.material.zeroize();
        self.cleared.store(self.material.expose() == &[0; 32], Ordering::SeqCst);
    }
}
struct PendingWrapping {
    cleared: Arc<AtomicBool>,
}
impl WrapDek for PendingWrapping {
    type Error = KeyOperationError;
    async fn wrap_dek(
        &self,
        _key: &KeyRef,
        dek: Dek,
        _binding: &AuthenticatedBinding,
    ) -> Result<WrappedDek, Self::Error> {
        let retained = RetainedMaterial {
            material: dek,
            cleared: self.cleared.clone(),
        };
        pending::<()>().await;
        drop(retained);
        Err(KeyOperationError::InternalFailure)
    }
}

// This proves owned material can be cleared on cancellation through the public
// future seam. It does not inspect freed memory or prove another adapter's erasure.
#[test]
fn pending_wrap_future_retains_owned_material_and_releases_it_on_cancellation() {
    let cleared = Arc::new(AtomicBool::new(false));
    let adapter = PendingWrapping {
        cleared: cleared.clone(),
    };
    let key = KeyRef::new("key").unwrap();
    let binding = AuthenticatedBinding::new(
        TenantId::new("tenant").unwrap(),
        ResourceTypeId::new("message").unwrap(),
        ResourceId::new("resource").unwrap(),
        KeyPurpose::new("purpose").unwrap(),
        BindingVersion::new(1).unwrap(),
    );
    {
        let mut future = pin!(adapter.wrap_dek(&key, Dek::from_bytes(Zeroizing::new([71; 32])), &binding));
        assert_eq!(
            future.as_mut().poll(&mut Context::from_waker(Waker::noop())),
            Poll::Pending
        );
        assert!(!cleared.load(Ordering::SeqCst));
    }
    assert!(cleared.load(Ordering::SeqCst));
}
