use super::*;

/// Rolling copy of an execution's recent stdout and stderr, so an
/// output-contains probe can be evaluated while the process still runs.
#[derive(Clone, Default)]
pub(super) struct OutputTap(pub(super) Arc<std::sync::Mutex<Vec<u8>>>);

impl OutputTap {
    pub(super) fn push(&self, chunk: &[u8]) {
        let mut buf = self.0.lock().unwrap_or_else(|e| e.into_inner());
        buf.extend_from_slice(chunk);
        if buf.len() > PROCESS_OUTPUT_TAIL {
            let excess = buf.len() - PROCESS_OUTPUT_TAIL;
            buf.drain(..excess);
        }
    }

    pub(super) fn contains(&self, needle: &[u8]) -> bool {
        let buf = self.0.lock().unwrap_or_else(|e| e.into_inner());
        !needle.is_empty() && buf.windows(needle.len()).any(|w| w == needle)
    }
}
