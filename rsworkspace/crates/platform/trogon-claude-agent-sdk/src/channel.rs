//! The queues between the caller and the runtime task: unbounded unless the caller opts into a capacity, as
//! neither upstream SDK bounds them.

use tokio::sync::mpsc::{self, error::TrySendError};

use crate::QueueCapacity;

pub(crate) enum Sender<T> {
    Bounded(mpsc::Sender<T>),
    Unbounded(mpsc::UnboundedSender<T>),
}

pub(crate) enum Receiver<T> {
    Bounded(mpsc::Receiver<T>),
    Unbounded(mpsc::UnboundedReceiver<T>),
}

pub(crate) fn channel<T>(capacity: Option<QueueCapacity>) -> (Sender<T>, Receiver<T>) {
    match capacity {
        Some(capacity) => {
            let (sender, receiver) = mpsc::channel(capacity.get());
            (Sender::Bounded(sender), Receiver::Bounded(receiver))
        }
        None => {
            let (sender, receiver) = mpsc::unbounded_channel();
            (Sender::Unbounded(sender), Receiver::Unbounded(receiver))
        }
    }
}

impl<T> Clone for Sender<T> {
    fn clone(&self) -> Self {
        match self {
            Self::Bounded(sender) => Self::Bounded(sender.clone()),
            Self::Unbounded(sender) => Self::Unbounded(sender.clone()),
        }
    }
}

impl<T> Sender<T> {
    /// Waits for room in a bounded queue; gives the value back when the receiver is gone.
    pub(crate) async fn send(&self, value: T) -> Result<(), T> {
        match self {
            Self::Bounded(sender) => sender.send(value).await.map_err(|error| error.0),
            Self::Unbounded(sender) => sender.send(value).map_err(|error| error.0),
        }
    }

    pub(crate) fn try_send(&self, value: T) -> Result<(), TrySendError<T>> {
        match self {
            Self::Bounded(sender) => sender.try_send(value),
            Self::Unbounded(sender) => sender.send(value).map_err(|error| TrySendError::Closed(error.0)),
        }
    }

    /// Resolves once a bounded queue has room; an unbounded queue always has room.
    pub(crate) async fn ready(&self) -> bool {
        match self {
            Self::Bounded(sender) => sender.reserve().await.is_ok(),
            Self::Unbounded(sender) => !sender.is_closed(),
        }
    }
}

impl<T> Receiver<T> {
    pub(crate) async fn recv(&mut self) -> Option<T> {
        match self {
            Self::Bounded(receiver) => receiver.recv().await,
            Self::Unbounded(receiver) => receiver.recv().await,
        }
    }

    pub(crate) fn close(&mut self) {
        match self {
            Self::Bounded(receiver) => receiver.close(),
            Self::Unbounded(receiver) => receiver.close(),
        }
    }
}
