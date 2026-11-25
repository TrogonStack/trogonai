use grpc_nats_micro::content_type::EncodeError;

/// Why a [`FakeSecretsService`](crate::FakeSecretsService) gave no reply.
#[derive(Debug, thiserror::Error)]
pub enum FakeRequestError {
    #[error("fake secrets service is unreachable")]
    Unreachable,
    #[error("fake secrets service reply did not encode")]
    Reply(#[source] EncodeError),
}
