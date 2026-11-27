/// A Vercel access token was refused.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
pub enum VercelTokenError {
    #[error("a Vercel token is required")]
    Empty,
    #[error("the Vercel token cannot be sent in an HTTP header")]
    NotAHeader,
}
