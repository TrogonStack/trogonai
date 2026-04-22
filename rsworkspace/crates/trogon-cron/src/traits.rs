use std::collections::HashSet;
use std::future::Future;

pub use trogon_nats::lease::{ReleaseLease, RenewLease, TryAcquireLease};

use crate::ResolvedJob;

pub trait LeaderLock:
    TryAcquireLease<Error = async_nats::jetstream::kv::CreateError>
    + RenewLease<Error = async_nats::jetstream::kv::UpdateError>
    + ReleaseLease<Error = async_nats::jetstream::kv::DeleteError>
    + Clone
    + Send
    + Sync
    + 'static
{
}

impl<T> LeaderLock for T where
    T: TryAcquireLease<Error = async_nats::jetstream::kv::CreateError>
        + RenewLease<Error = async_nats::jetstream::kv::UpdateError>
        + ReleaseLease<Error = async_nats::jetstream::kv::DeleteError>
        + Clone
        + Send
        + Sync
        + 'static
{
}

pub trait SchedulePublisher: Clone + Send + Sync + 'static {
    type Error: std::error::Error + Send + Sync + 'static;

    fn active_schedule_ids(&self) -> impl Future<Output = Result<HashSet<String>, Self::Error>> + Send;

    fn upsert_schedule(&self, job: &ResolvedJob) -> impl Future<Output = Result<(), Self::Error>> + Send;

    fn remove_schedule(&self, job_id: &str) -> impl Future<Output = Result<(), Self::Error>> + Send;
}
