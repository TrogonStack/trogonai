use async_trait::async_trait;

use crate::error::IndexerError;
use crate::event::{TrafficEvent, TrafficQueryFilter};

#[async_trait]
pub trait TrafficIndex: Send + Sync {
    async fn put_event(&self, event: TrafficEvent) -> Result<(), IndexerError>;

    async fn query(&self, filter: TrafficQueryFilter) -> Result<Vec<TrafficEvent>, IndexerError>;

    async fn tail(
        &self,
        tenant: &str,
        since: Option<chrono::DateTime<chrono::Utc>>,
        limit: u32,
    ) -> Result<Vec<TrafficEvent>, IndexerError>;
}

pub mod postgres;
