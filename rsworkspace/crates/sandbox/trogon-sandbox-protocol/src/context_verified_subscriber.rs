use std::pin::Pin;
use std::task::{Context, Poll};

use futures::Stream;

use crate::{VerifiedRequest, VerifiedSubscriber};

/// A [`VerifiedSubscriber`] that yields each admitted message together with its verified scope.
pub struct ContextVerifiedSubscriber {
    pub(crate) inner: VerifiedSubscriber,
}

impl Stream for ContextVerifiedSubscriber {
    type Item = VerifiedRequest;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            match Pin::new(&mut self.inner.inner).poll_next(cx) {
                Poll::Ready(Some(message)) => {
                    if let Some(scope) = self.inner.admit(&message) {
                        return Poll::Ready(Some(VerifiedRequest { message, scope }));
                    }
                }
                Poll::Ready(None) => return Poll::Ready(None),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}
