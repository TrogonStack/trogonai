use async_nats::Message;

/// One message the worker loop reacts to.
pub(crate) enum WorkerEvent {
    Lease(Message),
    Cancel(Message),
}
