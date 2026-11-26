use std::fmt;
use std::sync::Arc;

use trogon_sandbox_contract::RedactValues;

use crate::StreamRedactor;

/// What replaces every redacted occurrence.
pub const REDACTION_MARKER: &str = "[REDACTED]";

/// Shortest registered value, in bytes, that is redacted. Shorter values are dropped when a [`RedactionSet`] is built.
pub const MIN_REDACTABLE_LEN: usize = 6;

/// The values to redact, ready to filter output. Cheap to clone. An empty set is a no-op filter.
///
/// The sentinel design keeps real secrets out of the guest, but a literal `env` value or a file's contents can still
/// be echoed back by the command that received it. A caller registers those literals with a request, and a worker
/// runs everything it returns (stdout, stderr, streamed chunks, error text) through a set before it leaves the worker.
///
/// Limits, by design:
/// - Only the exact bytes of a registered value are replaced. A value the command encoded or transformed (base64, hex,
///   URL-escaped, split by whitespace, reversed, hashed) is not recognised.
/// - Values shorter than [`MIN_REDACTABLE_LEN`] bytes are ignored, because replacing a two-character string would
///   destroy unrelated output.
/// - Matching is leftmost-longest and non-overlapping: where two registered values start at the same byte the longer
///   one wins, and a value that starts inside an earlier match is not replaced separately.
/// - A redacted stream cannot be told from one that printed the marker.
///
/// Everything here is a pure function of the registered values and the bytes pushed, so the output never depends on
/// how a stream was chunked.
#[derive(Clone, Default)]
pub struct RedactionSet {
    // Longest first, deduplicated, none shorter than MIN_REDACTABLE_LEN.
    values: Arc<[Vec<u8>]>,
    pub(crate) max_len: usize,
}

impl fmt::Debug for RedactionSet {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "RedactionSet({} values)", self.values.len())
    }
}

impl RedactionSet {
    pub fn new<I, B>(values: I) -> Self
    where
        I: IntoIterator<Item = B>,
        B: AsRef<[u8]>,
    {
        let mut kept: Vec<Vec<u8>> = values
            .into_iter()
            .map(|value| value.as_ref().to_vec())
            .filter(|value| value.len() >= MIN_REDACTABLE_LEN)
            .collect();
        kept.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        kept.dedup();
        let max_len = kept.first().map_or(0, Vec::len);
        Self {
            values: kept.into(),
            max_len,
        }
    }

    /// The set for the values a request registered.
    pub fn from_request_values(values: &RedactValues) -> Self {
        Self::new(values.iter().map(|value| value.expose_secret().as_bytes()))
    }

    /// No value survives the minimum length, so filtering changes nothing.
    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    /// Redacts a whole buffer.
    pub fn redact(&self, data: &[u8]) -> Vec<u8> {
        if self.is_empty() {
            return data.to_vec();
        }
        let mut out = Vec::with_capacity(data.len());
        self.scan(data, data.len(), &mut out);
        out
    }

    /// Redacts text, for error messages and logs.
    ///
    /// Registered values are valid UTF-8 and the marker is ASCII, so the result is valid UTF-8.
    pub fn redact_str(&self, text: &str) -> String {
        String::from_utf8_lossy(&self.redact(text.as_bytes())).into_owned()
    }

    /// A filter for one stream that arrives in chunks.
    pub fn stream(&self) -> StreamRedactor {
        StreamRedactor::new(self.clone())
    }

    /// Scans `buf` from the start, appending to `out`, and decides every position below `decide_below`. Returns where
    /// it stopped: the first position it did not decide.
    pub(crate) fn scan(&self, buf: &[u8], decide_below: usize, out: &mut Vec<u8>) -> usize {
        let mut index = 0;
        while index < decide_below {
            match self.match_at(buf, index) {
                Some(length) => {
                    out.extend_from_slice(REDACTION_MARKER.as_bytes());
                    index += length;
                }
                None => {
                    out.push(buf[index]);
                    index += 1;
                }
            }
        }
        index
    }

    /// Length of the longest registered value that starts at `at`.
    fn match_at(&self, buf: &[u8], at: usize) -> Option<usize> {
        let rest = &buf[at..];
        self.values
            .iter()
            .find(|value| rest.starts_with(value))
            .map(|value| value.len())
    }
}
