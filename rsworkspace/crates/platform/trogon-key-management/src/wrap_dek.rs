use crate::{AuthenticatedBinding, Dek, KeyRef, WrappedDek};
use std::future::Future;

pub trait WrapDek {
    type Error: std::error::Error + Send + Sync + 'static;

    fn wrap_dek(
        &self,
        key: &KeyRef,
        dek: Dek,
        binding: &AuthenticatedBinding,
    ) -> impl Future<Output = Result<WrappedDek, Self::Error>> + Send;
}
