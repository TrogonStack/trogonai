use crate::FlyProviderError;

/// Turns a refused precondition into its error.
pub(crate) fn ensure(condition: bool, error: FlyProviderError) -> Result<(), FlyProviderError> {
    if condition { Ok(()) } else { Err(error) }
}
