use super::ConformanceCase;
use crate::KeyOperationError;

#[derive(Debug, thiserror::Error)]
pub enum ConformanceError<E: std::error::Error + 'static> {
    #[error("conformance fixtures must select distinct keys and bindings")]
    AliasedFixture,
    #[error("valid wrapping control failed")]
    Wrap(#[source] E),
    #[error("valid unwrap control failed")]
    Unwrap(#[source] E),
    #[error("round trip changed the data key")]
    PlaintextMismatch,
    #[error("another key accepted the wrapped value")]
    KeySubstitutionAccepted,
    #[error("another binding accepted the wrapped value")]
    BindingSubstitutionAccepted,
    #[error("negative case {case:?} returned an inadmissible refusal: {failure}")]
    UnexpectedRefusal {
        case: ConformanceCase,
        failure: KeyOperationError,
        #[source]
        source: E,
    },
    #[error("altered ciphertext was accepted")]
    TamperingAccepted,
}
