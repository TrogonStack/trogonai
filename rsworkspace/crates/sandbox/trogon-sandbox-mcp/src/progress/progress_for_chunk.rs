use rmcp::model::{ProgressNotificationParam, ProgressToken};
use trogon_sandbox_contract::{LogChunk, OutputStream};

/// The notification for one chunk. `progress` is the running chunk count, so it rises on every notification even
/// when a chunk is empty; the total is unknown. The message is the chunk text, with stderr marked.
pub(crate) fn progress_for_chunk(token: &ProgressToken, count: u64, chunk: &LogChunk) -> ProgressNotificationParam {
    let text = String::from_utf8_lossy(&chunk.data);
    let message = match chunk.stream {
        OutputStream::Stdout => text.into_owned(),
        OutputStream::Stderr => format!("[stderr] {text}"),
    };
    ProgressNotificationParam::new(token.clone(), count as f64).with_message(message)
}

#[cfg(test)]
mod tests;
