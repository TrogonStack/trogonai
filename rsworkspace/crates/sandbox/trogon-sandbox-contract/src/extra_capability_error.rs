/// A string is not a Linux capability a lease may keep.
#[derive(Debug, Clone, Eq, PartialEq, thiserror::Error)]
pub enum ExtraCapabilityError {
    #[error("`{0}` is not a valid capability name; use upper case letters, digits and `_`")]
    InvalidName(String),
    #[error("capability `{0}` can never be added by an override")]
    Forbidden(String),
}
