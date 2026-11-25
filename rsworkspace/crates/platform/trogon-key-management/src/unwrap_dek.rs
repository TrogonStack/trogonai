use crate::{AuthenticatedBinding, Dek, KeyRef, WrappedDek};
use std::future::Future;

pub trait UnwrapDek {
    type Error: std::error::Error + Send + Sync + 'static;

    fn unwrap_dek(
        &self,
        key: &KeyRef,
        wrapped: &WrappedDek,
        binding: &AuthenticatedBinding,
    ) -> impl Future<Output = Result<Dek, Self::Error>> + Send;
}
