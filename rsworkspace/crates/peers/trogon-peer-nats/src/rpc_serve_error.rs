#[derive(Debug, thiserror::Error)]
pub enum PeerRpcServeError {
    #[error("invalid peer RPC service name")]
    Name(#[from] grpc_nats_micro::ServiceNameError),
    #[error("invalid peer RPC service version")]
    Version(#[from] grpc_nats_micro::ServiceVersionError),
    #[error("invalid peer RPC prefix")]
    Prefix(#[from] grpc_nats_micro::SubjectPrefixError),
    #[error("invalid peer RPC method")]
    Method(#[from] grpc_nats_micro::MethodNameError),
    #[error("invalid peer RPC subscription")]
    Pattern(#[from] grpc_nats_micro::ServerSubjectPatternError),
    #[error("peer RPC service registration failed")]
    Serve(#[from] grpc_nats_micro::ServeError),
}
