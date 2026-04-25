use std::error::Error;
use std::future::Future;

use bytes::Bytes;

pub trait TryAcquireLease: Send + Sync + Clone + 'static {
    type Error: Error + Send + Sync;

    fn try_acquire(&self, value: Bytes) -> impl Future<Output = Result<u64, Self::Error>> + Send;
}

pub trait RenewLease: Send + Sync + Clone + 'static {
    type Error: Error + Send + Sync;

    fn renew(&self, value: Bytes, revision: u64) -> impl Future<Output = Result<u64, Self::Error>> + Send;
}

pub trait ReleaseLease: Send + Sync + Clone + 'static {
    type Error: Error + Send + Sync;

    fn release(&self, revision: u64) -> impl Future<Output = Result<(), Self::Error>> + Send;
}
