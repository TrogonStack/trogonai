use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader};

use crate::{Error, StderrTail};

pub(crate) struct FrameReader<R> {
    reader: BufReader<R>,
    buffer: Vec<u8>,
    limit: usize,
}

impl<R: AsyncRead + Unpin> FrameReader<R> {
    pub(crate) fn new(reader: R, limit: usize) -> Self {
        Self {
            reader: BufReader::new(reader),
            buffer: Vec::new(),
            limit,
        }
    }

    pub(crate) async fn next(&mut self) -> Result<Option<Vec<u8>>, Error> {
        loop {
            let available = self.reader.fill_buf().await.map_err(Error::Io)?;
            if available.is_empty() {
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

pub(crate) async fn drain_stderr<R: AsyncRead + Unpin>(
    mut reader: R,
    limit: usize,
) -> Result<StderrTail, std::io::Error> {
    let mut tail = Vec::with_capacity(limit.min(8192));
    let mut chunk = [0_u8; 8192];
    loop {
        let count = reader.read(&mut chunk).await?;
        if count == 0 {
            return Ok(StderrTail(tail));
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
