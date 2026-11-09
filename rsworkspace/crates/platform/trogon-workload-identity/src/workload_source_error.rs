#[derive(Debug, thiserror::Error)]
pub(crate) enum WorkloadSourceError {
    #[error("Workload API transport failed")]
    Transport(#[from] spiffe::transport::TransportError),
    #[error("Workload API stream failed")]
    Grpc(#[from] Box<tonic::Status>),
    #[error("Workload API transport is unavailable")]
    Ready(#[from] tonic::transport::Error),
}
