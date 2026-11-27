/// Turns a refused precondition into its error.
pub(crate) fn ensure<E>(condition: bool, error: E) -> Result<(), E> {
    if condition { Ok(()) } else { Err(error) }
}
