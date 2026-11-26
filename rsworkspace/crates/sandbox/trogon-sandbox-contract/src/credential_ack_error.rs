/// A credential ack does not satisfy the rules [`CredentialAck`](crate::CredentialAck) enforces.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum CredentialAckError {
    #[error("a credential ack must carry `ok: true`")]
    NotOk,
}
