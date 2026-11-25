#[derive(Debug, thiserror::Error)]
pub enum RuntimeConfigError {
    #[error("peer RPC deadline is invalid")]
    Timeout(#[source] trogon_peer_nats::PeerRequestTimeoutError),
    #[error("required peer setting is missing: {0}")]
    Missing(&'static str),
    #[error("peer routing setting is invalid")]
    Routing(#[source] trogon_peer_nats::RoutingTokenError),
    #[error("peer identity setting is invalid")]
    Identity(#[source] trogon_peer::PeerValueError),
    #[error("peer routing mode is invalid")]
    Mode,
    #[error("peer availability observation is invalid")]
    Availability,
    #[error("peer connector requires NATS credentials")]
    Anonymous,
}
