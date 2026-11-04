use trogon_peer::EvolveError;
use trogonai_proto::peers::PeerEventPayloadError;

#[derive(Debug, thiserror::Error)]
pub enum HistoryError<ReadError> {
    #[error("peer history read failed: {0}")]
    Read(#[source] ReadError),
    #[error("peer history exceeds the bounded replay limit")]
    Limit,
    #[error("peer history event decoding failed: {0}")]
    Decode(#[source] PeerEventPayloadError),
    #[error("peer history contains an unknown event")]
    UnknownEvent,
    #[error("peer history replay failed: {0}")]
    Evolve(#[source] EvolveError),
}
