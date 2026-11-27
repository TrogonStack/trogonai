/// A Vercel project id was refused.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
#[error("a Vercel project id is required")]
pub struct VercelProjectIdError;
