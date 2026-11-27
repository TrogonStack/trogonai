use async_nats::Message;

use crate::K8sVerb;

/// One message the worker loop reacts to.
pub(crate) enum WorkerEvent {
    Lease(Message),
    Cancel(Message),
    SnapshotExport(Message),
    Verb(K8sVerb, Message),
}
