use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader};

use crate::{Error, StderrTail, error::JsonDecodeError};

pub(crate) struct FrameReader<R> {
    reader: BufReader<R>,
    buffer: Vec<u8>,
    /// Characters held in `buffer`, which is what Python's `len(str)` guard counts.
    chars: usize,
    limit: Option<usize>,
    tail: bool,
}

/// Python counts the decoded characters of a line: every byte that is not a UTF-8 continuation byte.
fn chars_in(bytes: &[u8]) -> usize {
    bytes.iter().filter(|byte| (**byte & 0xC0) != 0x80).count()
}

fn guard(limit: Option<usize>, length: usize) -> Result<(), Error> {
    match limit {
        Some(limit) if length > limit => Err(Error::JsonDecode(JsonDecodeError::buffer_exceeded(limit, length))),
        _ => Ok(()),
    }
}

impl<R: AsyncRead + Unpin> FrameReader<R> {
    /// `limit` is Python's `max_buffer_size`; TypeScript has no such bound, so `None` keeps every line.
    pub(crate) fn new(reader: R, limit: Option<usize>) -> Self {
        Self {
            reader: BufReader::new(reader),
            buffer: Vec::new(),
            chars: 0,
            limit,
            tail: false,
        }
    }

    /// Whether the last frame was the unterminated rest of the stream, cut off by end of file.
    pub(crate) fn last_was_tail(&self) -> bool {
        self.tail
    }

    pub(crate) async fn next(&mut self) -> Result<Option<Vec<u8>>, Error> {
        self.tail = false;
        loop {
            let available = self.reader.fill_buf().await.map_err(Error::Io)?;
            if available.is_empty() {
                self.tail = !self.buffer.is_empty();
                self.chars = 0;
                return Ok(if self.buffer.is_empty() {
                    None
                } else {
                    Some(std::mem::take(&mut self.buffer))
                });
            }
            let newline = available.iter().position(|byte| *byte == b'\n');
            let count = newline.map_or(available.len(), |index| index + 1);
            // A finished line is measured without its newline; an unfinished one by everything received.
            let length = self
                .chars
                .saturating_add(chars_in(&available[..newline.unwrap_or(count)]));
            guard(self.limit, length)?;
            self.buffer.extend_from_slice(&available[..count]);
            self.chars = length;
            self.reader.consume(count);
            if newline.is_some() {
                self.buffer.pop();
                self.chars = 0;
                if self.buffer.last() == Some(&b'\r') {
                    self.buffer.pop();
                }
                if !self.buffer.is_empty() {
                    return Ok(Some(std::mem::take(&mut self.buffer)));
                }
            }
        }
    }
}

fn deliver(handler: &crate::StderrHandler, text: &str) {
    // A failing callback must not break the stream, as in both upstream SDKs.
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| (handler.callback)(text)));
}

fn deliver_lines(handler: &crate::StderrHandler, pending: &mut Vec<u8>, flush: bool) {
    while let Some(position) = pending.iter().position(|byte| *byte == b'\n') {
        let line: Vec<u8> = pending.drain(..=position).collect();
        deliver_line(handler, &line);
    }
    if flush && !pending.is_empty() {
        let line = std::mem::take(pending);
        deliver_line(handler, &line);
    }
}

fn deliver_line(handler: &crate::StderrHandler, line: &[u8]) {
    let text = String::from_utf8_lossy(line);
    let trimmed = text.trim_end();
    if !trimmed.is_empty() {
        deliver(handler, trimmed);
    }
}

/// The stderr read so far, readable while the drain still runs so a stream left open does not lose it.
#[derive(Clone, Default)]
pub(crate) struct SharedTail {
    bytes: std::sync::Arc<std::sync::Mutex<Vec<u8>>>,
    units: usize,
}

impl SharedTail {
    pub(crate) fn new(units: usize) -> Self {
        Self {
            bytes: std::sync::Arc::default(),
            units,
        }
    }

    /// The last UTF-16 code units of what has been read, as TypeScript's `slice(-2048)` keeps them.
    pub(crate) fn snapshot(&self) -> StderrTail {
        let bytes = self.bytes.lock().map(|bytes| bytes.clone()).unwrap_or_default();
        StderrTail(last_units(&bytes, self.units))
    }

    fn append(&self, chunk: &[u8]) {
        // A UTF-16 code unit takes at most three UTF-8 bytes; the slack covers a character cut at the front.
        let limit = self
            .units
            .saturating_mul(3)
            .saturating_add(4 * usize::from(self.units > 0));
        let Ok(mut tail) = self.bytes.lock() else {
            return;
        };
        if chunk.len() >= limit {
            tail.clear();
            tail.extend_from_slice(&chunk[chunk.len() - limit..]);
        } else {
            let excess = tail.len().saturating_add(chunk.len()).saturating_sub(limit);
            tail.drain(..excess);
            tail.extend_from_slice(chunk);
        }
    }
}

/// Reads stderr to its end into `tail` and delivers it to the handler. A partial line longer than `line_limit`
/// bytes is delivered without waiting for its newline.
pub(crate) async fn drain_stderr_with<R: AsyncRead + Unpin>(
    mut reader: R,
    tail: SharedTail,
    line_limit: usize,
    handler: Option<crate::StderrHandler>,
) -> Result<(), std::io::Error> {
    let mut pending = Vec::new();
    let mut chunk = [0_u8; 8192];
    loop {
        let count = reader.read(&mut chunk).await?;
        if count == 0 {
            if let Some(handler) = &handler
                && handler.delivery == crate::StderrDelivery::Lines
            {
                deliver_lines(handler, &mut pending, true);
            }
            return Ok(());
        }
        if let Some(handler) = &handler {
            match handler.delivery {
                crate::StderrDelivery::Chunks => deliver(handler, &String::from_utf8_lossy(&chunk[..count])),
                crate::StderrDelivery::Lines => {
                    pending.extend_from_slice(&chunk[..count]);
                    deliver_lines(handler, &mut pending, false);
                    // Python flushes a partial line once it exceeds the buffer limit.
                    if pending.len() > line_limit {
                        deliver_lines(handler, &mut pending, true);
                    }
                }
            }
        }
        tail.append(&chunk[..count]);
    }
}

/// The last `units` UTF-16 code units of the lossily decoded bytes. A surrogate pair cut in half leaves a lone low
/// surrogate in JavaScript, which UTF-8 cannot carry, so it is dropped.
fn last_units(bytes: &[u8], units: usize) -> Vec<u8> {
    let text = String::from_utf8_lossy(bytes);
    let mut kept = 0_usize;
    let mut start = text.len();
    for (index, character) in text.char_indices().rev() {
        let width = character.len_utf16();
        if kept + width > units {
            break;
        }
        kept += width;
        start = index;
    }
    // TypeScript trims the kept tail with `String.prototype.trim`, which also strips the byte order mark.
    text[start..]
        .trim_matches(|character: char| character.is_whitespace() || character == '\u{feff}')
        .as_bytes()
        .to_vec()
}

#[cfg(test)]
mod tests;
