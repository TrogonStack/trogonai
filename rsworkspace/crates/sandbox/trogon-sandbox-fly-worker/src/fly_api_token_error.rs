/// A Fly API token was refused.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum FlyApiTokenError {
    #[error("a Fly API token is required")]
    Empty,
    #[error("the Fly API token cannot be sent in an HTTP header")]
    NotAHeader,
}
