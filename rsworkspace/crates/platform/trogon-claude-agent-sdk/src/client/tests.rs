use std::{
    future::Future,
    pin::pin,
    task::{Context, Poll, Waker},
};

use super::*;

/// Connect cannot create a client until the runtime exists, so construct it here.
#[test]
fn disconnect_reports_unimplemented_runtime() {
    let client = ClaudeSdkClient { _private: () };
    let mut disconnect = pin!(client.disconnect());
    let mut context = Context::from_waker(Waker::noop());
    assert!(matches!(
        disconnect.as_mut().poll(&mut context),
        Poll::Ready(Err(Error::NotImplemented))
    ));
}
