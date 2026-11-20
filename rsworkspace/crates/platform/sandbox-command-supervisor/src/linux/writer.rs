use super::*;

pub(super) struct Pending {
    bytes: Vec<u8>,
    offset: usize,
    stream: Option<Stream>,
}

pub(super) struct Writer {
    pub(super) pending: VecDeque<Pending>,
    pub(super) data_bytes: usize,
    pub(super) control_bytes: usize,
    pub(super) closed: bool,
    pub(super) stdout_truncated: bool,
    pub(super) stderr_truncated: bool,
}

impl Writer {
    pub(super) fn new() -> Self {
        Self {
            pending: VecDeque::new(),
            data_bytes: 0,
            control_bytes: 0,
            closed: false,
            stdout_truncated: false,
            stderr_truncated: false,
        }
    }

    pub(super) fn truncate(&mut self, stream: Stream) {
        match stream {
            Stream::Stdout => self.stdout_truncated = true,
            Stream::Stderr => self.stderr_truncated = true,
        }
    }

    pub(super) fn queue(&mut self, event: Event) -> Result<(), ErrorCode> {
        let stream = match &event {
            Event::Output { stream, .. } => Some(*stream),
            _ => None,
        };
        let bytes = encode_event(&event).map_err(|_| ErrorCode::InvalidFrame)?;
        if self.closed || stream.is_some_and(|_| self.data_bytes + bytes.len() > QUEUED_DATA_LIMIT) {
            if let Some(stream) = stream {
                self.truncate(stream);
            }
            return Ok(());
        }
        if stream.is_none() && self.control_bytes + bytes.len() > QUEUED_CONTROL_LIMIT {
            return Err(ErrorCode::IoFailure);
        }
        if stream.is_some() {
            self.data_bytes += bytes.len();
        } else {
            self.control_bytes += bytes.len();
        }
        self.pending.push_back(Pending {
            bytes,
            offset: 0,
            stream,
        });
        Ok(())
    }

    pub(super) fn flush(&mut self) {
        while let Some(front) = self.pending.front_mut() {
            let remaining = &front.bytes[front.offset..];
            let written = unsafe { libc::write(libc::STDOUT_FILENO, remaining.as_ptr().cast(), remaining.len()) };
            if written < 0 {
                match errno() {
                    libc::EAGAIN | libc::EINTR => return,
                    _ => {
                        self.closed = true;
                        self.pending.clear();
                        self.data_bytes = 0;
                        self.control_bytes = 0;
                        return;
                    }
                }
            }
            if written == 0 {
                return;
            }
            front.offset += written as usize;
            if front.offset == front.bytes.len()
                && let Some(completed) = self.pending.pop_front()
            {
                if completed.stream.is_some() {
                    self.data_bytes -= completed.bytes.len();
                } else {
                    self.control_bytes -= completed.bytes.len();
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
