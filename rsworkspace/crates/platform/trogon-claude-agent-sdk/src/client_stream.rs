//! The message reader as a `futures_util::Stream`: TypeScript `for await (const message of query)` and Python
//! `receive_messages()` / `receive_response()`.
//!
//! The iterator methods of the TypeScript `Query` map to the Rust lifecycle: `return()`, a `for await` that
//! breaks and `Symbol.asyncDispose` close the process, wait for cleanup and discard what was not read yet, which
//! is [`Query::disconnect`]; `throw(error)` does the same and then rejects with the caller's own error, which the
//! caller returns after `disconnect`, or [`ClaudeSdkClient::abort`] when the process should stop at once. Dropping
//! the stream closes the process without waiting.

use std::{
    future::Future,
    pin::Pin,
    task::{Context, Poll, ready},
};

use futures_util::Stream;

use super::{ClaudeSdkClient, ClientRuntime, Query};
use crate::{Error, Message};

impl ClientRuntime {
    /// Waits for the terminal outcome and then the driver task. Each is taken once it resolved, so polling again
    /// after completion returns `Ok(())` instead of polling a finished future.
    pub(super) fn poll_finish(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Error>> {
        if let Some(terminal) = self.terminal.as_mut() {
            let outcome = ready!(Pin::new(terminal).poll(cx));
            self.terminal = None;
            match outcome {
                Ok(outcome) => self.outcome = Some(outcome),
                Err(_) => return Poll::Ready(Err(Error::DriverStopped)),
            }
        }
        if let Some(driver) = self.driver.as_mut() {
            let joined = ready!(Pin::new(driver).poll(cx));
            self.driver = None;
            if let Err(error) = joined {
                self.outcome = None;
                return Poll::Ready(Err(Error::DriverTask(error)));
            }
        }
        Poll::Ready(self.outcome.take().unwrap_or(Ok(())))
    }
}

impl ClaudeSdkClient {
    /// [`ClaudeSdkClient::next_message`] as a poll: `None` once the CLI's output ended and cleanup finished.
    pub(super) fn poll_next_message(&mut self, cx: &mut Context<'_>) -> Poll<Result<Option<Message>, Error>> {
        match ready!(self.runtime.messages.poll_recv(cx)) {
            Some(message) => Poll::Ready(Ok(Some(message))),
            None => self.runtime.poll_finish(cx).map_ok(|()| None),
        }
    }

    /// Python `receive_messages()`: every message until the CLI's output ends, as [`ClaudeSdkClient::next_message`]
    /// reads them. The client is itself a `Stream`; this borrows it so the client stays usable afterwards.
    pub fn receive_messages(&mut self) -> impl Stream<Item = Result<Message, Error>> + '_ {
        futures_util::stream::poll_fn(move |cx| self.poll_next_message(cx).map(Result::transpose))
    }

    /// Python `receive_response()`: messages up to and including the next `Message::Result`, then the stream
    /// ends. It also ends after an error or when the CLI's output ends first. Messages after the result stay
    /// queued for the next read.
    pub fn receive_response(&mut self) -> impl Stream<Item = Result<Message, Error>> + '_ {
        let mut done = false;
        futures_util::stream::poll_fn(move |cx| {
            if done {
                return Poll::Ready(None);
            }
            let item = ready!(self.poll_next_message(cx)).transpose();
            done = !matches!(&item, Some(Ok(message)) if !matches!(message, Message::Result(_)));
            Poll::Ready(item)
        })
    }
}

/// TypeScript `for await (const message of query)` over the connection's messages. Ends where
/// [`ClaudeSdkClient::next_message`] returns `None`; a cleanup error is the last item.
impl Stream for ClaudeSdkClient {
    type Item = Result<Message, Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().poll_next_message(cx).map(Result::transpose)
    }
}

impl Query {
    /// [`Query::next_message`] as a poll. A `query_turn` query closes the process after its result and ends once
    /// cleanup finished.
    pub(super) fn poll_next_message(&mut self, cx: &mut Context<'_>) -> Poll<Result<Option<Message>, Error>> {
        let Some(client) = self.client.as_mut() else {
            return Poll::Ready(Ok(None));
        };
        if self.one_turn && self.result_received {
            client.close();
            let outcome = ready!(client.runtime.poll_finish(cx));
            self.client = None;
            return Poll::Ready(outcome.map(|()| None));
        }
        let message = ready!(client.poll_next_message(cx))?;
        if matches!(message, Some(Message::Result(_))) {
            self.result_received = true;
        }
        if let (Some(sse), Some(message)) = (&self.sse, &message) {
            sse.mark_delivered(message.raw());
        }
        Poll::Ready(Ok(message))
    }
}

/// TypeScript `for await (const message of query)`. Ends where [`Query::next_message`] returns `None`; a cleanup
/// error is the last item. Breaking out of the loop and then calling [`Query::disconnect`] is what the TypeScript
/// loop's implicit `return()` does.
impl Stream for Query {
    type Item = Result<Message, Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.get_mut().poll_next_message(cx).map(Result::transpose)
    }
}
