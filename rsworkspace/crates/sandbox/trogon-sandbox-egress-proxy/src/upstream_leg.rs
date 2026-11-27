use std::future::Future;
use std::io;
use std::pin::Pin;

use std::task::{Context, Waker};

use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

use crate::http_intercept_error::HttpInterceptError;
use crate::stream_use::StreamUse;

pub(crate) type Dial<S> =
    Box<dyn FnMut() -> Pin<Box<dyn Future<Output = Result<S, HttpInterceptError>> + Send>> + Send>;

/// The upstream side of one client connection: dialed when the first request
/// needs it, and dialed again when the upstream closed the connection while it
/// sat idle between requests, as a keep-alive agent does.
pub(crate) struct UpstreamLeg<S> {
    stream: Option<S>,
    dial: Option<Dial<S>>,
    stream_use: StreamUse,
}

impl<S> UpstreamLeg<S>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    pub(crate) fn connected(stream: S) -> Self {
        Self {
            stream: Some(stream),
            dial: None,
            stream_use: StreamUse::Fresh,
        }
    }

    pub(crate) fn on_demand(dial: Dial<S>) -> Self {
        Self {
            stream: None,
            dial: Some(dial),
            stream_use: StreamUse::Fresh,
        }
    }

    /// Drops the connection, which cannot carry another request. `Ends` when
    /// there is no other to dial.
    pub(crate) fn can_redial(&self) -> bool {
        self.dial.is_some()
    }

    pub(crate) fn discard(&mut self) -> AfterDiscard {
        self.stream = None;
        match self.dial {
            Some(_) => AfterDiscard::Redials,
            None => AfterDiscard::Ends,
        }
    }

    /// The connection the next request goes out on, and whether it already
    /// carried one.
    pub(crate) async fn stream(&mut self) -> Result<(&mut S, StreamUse), HttpInterceptError> {
        if self.dial.is_some() && self.stream.as_mut().is_some_and(has_closed) {
            self.stream = None;
        }
        let stream = match self.stream.take() {
            Some(stream) => stream,
            None => {
                let Some(dial) = self.dial.as_mut() else {
                    return Err(HttpInterceptError::UpstreamConnect(io::Error::new(
                        io::ErrorKind::NotConnected,
                        "no upstream connection and nothing to dial",
                    )));
                };
                self.stream_use = StreamUse::Fresh;
                dial().await?
            }
        };
        let stream_use = self.stream_use;
        Ok((self.stream.insert(stream), stream_use))
    }

    /// Marks the connection as having carried an exchange it can be reused
    /// after.
    pub(crate) fn served(&mut self) {
        self.stream_use = StreamUse::Reused;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AfterDiscard {
    Redials,
    Ends,
}

/// An idle upstream has nothing to say, so anything readable right now (an end
/// of stream, a reset or stray bytes) means it is no longer usable.
fn has_closed<S: AsyncRead + Unpin>(stream: &mut S) -> bool {
    let mut probe = [0u8; 1];
    let mut buf = ReadBuf::new(&mut probe);
    let mut cx = Context::from_waker(Waker::noop());
    Pin::new(stream).poll_read(&mut cx, &mut buf).is_ready()
}

#[cfg(test)]
mod tests;
