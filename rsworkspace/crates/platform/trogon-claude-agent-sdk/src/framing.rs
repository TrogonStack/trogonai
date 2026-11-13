use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader};

use crate::{Error, StderrTail};

pub(crate) struct FrameReader<R> {
    reader: BufReader<R>,
    buffer: Vec<u8>,
    limit: usize,
    tail: bool,
}

impl<R: AsyncRead + Unpin> FrameReader<R> {
    pub(crate) fn new(reader: R, limit: usize) -> Self {
        Self {
            reader: BufReader::new(reader),
            buffer: Vec::new(),
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
                return Ok(if self.buffer.is_empty() {
                    None
                } else {
                    Some(std::mem::take(&mut self.buffer))
                });
            }
            let newline = available.iter().position(|byte| *byte == b'\n');
            let count = newline.map_or(available.len(), |index| index + 1);
            if self.buffer.len().saturating_add(count) > self.limit {
                return Err(Error::FrameTooLarge { limit: self.limit });
            }
            self.buffer.extend_from_slice(&available[..count]);
            self.reader.consume(count);
            if newline.is_some() {
                self.buffer.pop();
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

#[cfg(test)]
pub(crate) async fn drain_stderr<R: AsyncRead + Unpin>(reader: R, limit: usize) -> Result<StderrTail, std::io::Error> {
    drain_stderr_with(reader, limit, None).await
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

pub(crate) async fn drain_stderr_with<R: AsyncRead + Unpin>(
    mut reader: R,
    limit: usize,
    handler: Option<crate::StderrHandler>,
) -> Result<StderrTail, std::io::Error> {
    let mut tail = Vec::with_capacity(limit.min(8192));
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
            return Ok(StderrTail(tail));
        }
        if let Some(handler) = &handler {
            match handler.delivery {
                crate::StderrDelivery::Chunks => deliver(handler, &String::from_utf8_lossy(&chunk[..count])),
                crate::StderrDelivery::Lines => {
                    pending.extend_from_slice(&chunk[..count]);
                    deliver_lines(handler, &mut pending, false);
                    // Python flushes a partial line once it exceeds the buffer limit.
                    if pending.len() > limit {
                        deliver_lines(handler, &mut pending, true);
                    }
                }
            }
        }
        if count >= limit {
            tail.clear();
            tail.extend_from_slice(&chunk[count - limit..count]);
        } else {
            let excess = tail.len().saturating_add(count).saturating_sub(limit);
            tail.drain(..excess);
            tail.extend_from_slice(&chunk[..count]);
        }
    }
}
