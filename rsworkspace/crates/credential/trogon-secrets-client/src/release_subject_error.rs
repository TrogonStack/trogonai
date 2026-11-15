use grpc_nats_micro::{EndpointSubjectError, MethodNameError, ServiceNameError, SubjectPrefixError};

/// Why the release subject could not be derived for a call.
#[derive(Debug, thiserror::Error)]
pub enum ReleaseSubjectError {
    #[error("release subject prefix is invalid")]
    Prefix(#[source] SubjectPrefixError),
    #[error("release service name is invalid")]
    Service(#[source] ServiceNameError),
    #[error("release method name is invalid")]
    Method(#[source] MethodNameError),
    #[error("release subject is invalid")]
    Endpoint(#[source] EndpointSubjectError),
}
