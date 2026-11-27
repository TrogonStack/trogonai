use std::sync::{Arc, Mutex};
use std::time::Duration;

#[allow(deprecated)]
use rmcp::model::LoggingMessageNotificationParam;
use rmcp::model::ProgressNotificationParam;
use rmcp::service::NotificationContext;
use rmcp::{ClientHandler, RoleClient};
use serde_json::Value;

use crate::lock::lock;

/// A client that keeps the progress messages and the evidence logging data the server sends.
#[derive(Clone, Default)]
pub(crate) struct Recorder {
    progress: Arc<Mutex<Vec<String>>>,
    logging: Arc<Mutex<Vec<Value>>>,
}

impl Recorder {
    /// The progress messages once `count` have arrived, or whatever arrived by the deadline.
    pub(crate) async fn progress(&self, count: usize) -> Vec<String> {
        self.settled(count, || lock(&self.progress).clone()).await
    }

    /// The logging data once `count` entries have arrived, or whatever arrived by the deadline.
    pub(crate) async fn logging(&self, count: usize) -> Vec<Value> {
        self.settled(count, || lock(&self.logging).clone()).await
    }

    async fn settled<T>(&self, count: usize, read: impl Fn() -> Vec<T>) -> Vec<T> {
        for _ in 0..200 {
            let seen = read();
            if seen.len() >= count {
                return seen;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        read()
    }
}

#[allow(deprecated)]
impl ClientHandler for Recorder {
    async fn on_progress(&self, params: ProgressNotificationParam, _context: NotificationContext<RoleClient>) {
        lock(&self.progress).push(params.message.unwrap_or_default());
    }

    async fn on_logging_message(
        &self,
        params: LoggingMessageNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        lock(&self.logging).push(params.data);
    }
}

#[cfg(test)]
mod tests;
