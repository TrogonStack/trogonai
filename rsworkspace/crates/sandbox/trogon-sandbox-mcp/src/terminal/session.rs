use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::StreamExt;
use futures::stream::BoxStream;
use tokio::sync::Notify;
use tokio::task::JoinHandle;
use trogon_sandbox_contract::TerminalOutput;

use super::ReadOutcome;
use super::buffer::Buffer;
use crate::lock::lock;

/// One terminal's buffered output and the task that fills the buffer.
pub(crate) struct Session {
    buffer: Mutex<Buffer>,
    changed: Notify,
    pump: Mutex<Option<JoinHandle<()>>>,
}

impl Session {
    pub(super) fn start(output: BoxStream<'static, TerminalOutput>) -> Arc<Self> {
        let session = Arc::new(Self {
            buffer: Mutex::new(Buffer::default()),
            changed: Notify::new(),
            pump: Mutex::new(None),
        });
        let weak = Arc::downgrade(&session);
        let pump = tokio::spawn(async move {
            let mut output = output;
            while let Some(frame) = output.next().await {
                let Some(session) = weak.upgrade() else { return };
                let ended = matches!(frame, TerminalOutput::Exit { .. });
                lock(&session.buffer).push(frame);
                session.changed.notify_waiters();
                if ended {
                    return;
                }
            }
            if let Some(session) = weak.upgrade() {
                lock(&session.buffer).close();
                session.changed.notify_waiters();
            }
        });
        *lock(&session.pump) = Some(pump);
        session
    }

    /// Returns what is buffered, waiting up to `wait` for something when nothing is.
    pub(crate) async fn read(&self, max_bytes: usize, wait: Duration) -> ReadOutcome {
        let deadline = tokio::time::Instant::now() + wait;
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            changed.as_mut().enable();
            {
                let mut buffer = lock(&self.buffer);
                if buffer.ready() || tokio::time::Instant::now() >= deadline {
                    return buffer.take(max_bytes.max(1));
                }
            }
            let _ = tokio::time::timeout_at(deadline, changed).await;
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Some(pump) = lock(&self.pump).take() {
            pump.abort();
        }
    }
}

#[cfg(test)]
mod tests;
