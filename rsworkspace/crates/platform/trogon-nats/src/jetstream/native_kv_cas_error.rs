use super::NativeKvCasStage;
use async_nats::jetstream::context;

#[derive(thiserror::Error)]
pub enum NativeKvCasError {
    #[error("conditional KV scope is not an exact native bucket binding")]
    InvalidBinding,
    #[error("conditional KV key is outside its bounded native profile")]
    InvalidKey,
    #[error("conditional KV key/global sequence expectation is inconsistent")]
    InvalidExpectation,
    #[error("conditional KV message exceeds its envelope limit")]
    EnvelopeLimit,
    #[error("conditional KV global or key sequence changed")]
    Conflict,
    #[error("conditional KV acknowledgement does not prove the exact write")]
    InvalidAcknowledgement,
    #[error("conditional KV bucket lookup failed before publication")]
    Lookup(#[source] context::KeyValueError),
    #[error("conditional KV publication is uncertain at {stage:?}")]
    Uncertain {
        stage: NativeKvCasStage,
        #[source]
        source: context::PublishError,
    },
}
impl std::fmt::Debug for NativeKvCasError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Uncertain { stage, .. } => formatter
                .debug_struct("Uncertain")
                .field("stage", stage)
                .finish_non_exhaustive(),
            _ => write!(formatter, "{self}"),
        }
    }
}
