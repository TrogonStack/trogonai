use async_nats::Message;

use crate::RoutedVerb;

/// One message the worker loop reacts to.
pub(crate) enum WorkerEvent {
    Lease(Message),
    Cancel(Message),
    Verb(RoutedVerb, Message),
}
