#[derive(Debug, thiserror::Error)]
pub enum PeerRpcClientError {
    #[error("peer request encoding failed")]
    Encode(#[from] grpc_nats_micro::content_type::EncodeError),
    #[error("peer request transport failed")]
    Request(#[from] crate::peer_request_error::PeerRequestError),
    #[error("peer reply failed")]
    Reply(#[from] grpc_nats_micro::ReplyError),
}
