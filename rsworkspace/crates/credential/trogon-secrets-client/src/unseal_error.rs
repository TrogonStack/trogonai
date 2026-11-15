/// Why a sealed value did not open as a credential.
///
/// None of them quotes the value: a sealed value that opened but is not a
/// credential is still a secret.
#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum UnsealError {
    /// The value was not sealed to this request's xkey by the pinned service
    /// xkey, or was altered on the way.
    #[error("sealed value was not sealed to this request by the pinned service xkey")]
    NotOpened,
    #[error("opened value is not UTF-8")]
    NotUtf8,
    #[error("opened value is empty")]
    Empty,
}
