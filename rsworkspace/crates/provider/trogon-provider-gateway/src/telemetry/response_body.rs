use std::{
    pin::Pin,
    task::{Context, Poll},
};

use axum::body::{Body, Bytes};
use http_body::{Body as HttpBody, Frame, SizeHint};
use pin_project_lite::pin_project;

use super::{request_completion::RequestCompletion, request_record::RequestRecord};

pin_project! {
    pub(super) struct ResponseBody {
        #[pin]
        inner: Body,
        record: Option<RequestRecord>,
    }
}

impl ResponseBody {
    pub(super) fn new(inner: Body, record: RequestRecord) -> Self {
        let record = if inner.is_end_stream() {
            record.finish(RequestCompletion::Complete);
            None
        } else {
            Some(record)
        };
        Self { inner, record }
    }
}

impl HttpBody for ResponseBody {
    type Data = Bytes;
    type Error = axum::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        context: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        let projected = self.project();
        let span = projected.record.as_ref().map(|record| record.span.clone());
        let _entered = span.as_ref().map(|span| span.enter());
        let mut inner = projected.inner;
        let frame = inner.as_mut().poll_frame(context);
        let completion = match &frame {
            Poll::Ready(Some(Err(_))) => Some(RequestCompletion::BodyError),
            Poll::Ready(None) => Some(RequestCompletion::Complete),
            Poll::Ready(Some(Ok(_))) if inner.is_end_stream() => Some(RequestCompletion::Complete),
            _ => None,
        };
        if let Some(completion) = completion
            && let Some(record) = projected.record.take()
        {
            record.finish(completion);
        }
        frame
    }

    fn is_end_stream(&self) -> bool {
        self.inner.is_end_stream()
    }

    fn size_hint(&self) -> SizeHint {
        self.inner.size_hint()
    }
}
