#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeerOperation {
    Heartbeat,
    Detach,
    Send,
    Status,
    Prepare,
    Acknowledge,
    Roster,
    Reconcile,
    ReadPeer,
}
