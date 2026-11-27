use crate::ProxmoxProviderError;

/// Turns a refused precondition into its error.
pub(crate) fn ensure(condition: bool, error: ProxmoxProviderError) -> Result<(), ProxmoxProviderError> {
    if condition { Ok(()) } else { Err(error) }
}
