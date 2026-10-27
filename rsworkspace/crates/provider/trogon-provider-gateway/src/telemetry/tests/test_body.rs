use std::{
    future::Future,
    io,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use axum::body::Bytes;
use http_body::{Body, Frame};
use tokio::sync::{Mutex, oneshot};

pub(super) enum TestBody {
    Streaming {
        release: oneshot::Receiver<()>,
        data: Option<Bytes>,
    },
    Failing,
}

impl TestBody {
    pub(super) fn streaming() -> (oneshot::Sender<()>, Self) {
        let (release, receiver) = oneshot::channel();
        (
            release,
            Self::Streaming {
                release: receiver,
                data: Some(Bytes::from_static(b"streamed")),
            },
        )
    }

    pub(super) const fn failing() -> Self {
        Self::Failing
    }

    pub(super) fn into_router(self) -> axum::Router {
        let body = Arc::new(Mutex::new(Some(self)));
        axum::Router::new().fallback(move || {
            let body = Arc::clone(&body);
            async move { axum::body::Body::new(body.lock().await.take().unwrap()) }
        })
    }
}

impl Body for TestBody {
    type Data = Bytes;
    type Error = io::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        match self.get_mut() {
            Self::Streaming { release, data } => {
                if data.is_none() {
                    return Poll::Ready(None);
                }
                match Pin::new(release).poll(context) {
                    Poll::Ready(_) => Poll::Ready(data.take().map(|data| Ok(Frame::data(data)))),
                    Poll::Pending => Poll::Pending,
                }
            }
            Self::Failing => Poll::Ready(Some(Err(io::Error::other("confidential body error detail")))),
        }
    }

    fn is_end_stream(&self) -> bool {
        matches!(self, Self::Streaming { data: None, .. })
    }
}
