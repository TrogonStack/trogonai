/// A Vercel team id was refused.
#[derive(Debug, Clone, Copy, Eq, PartialEq, thiserror::Error)]
#[error("a Vercel team id is required")]
pub struct VercelTeamIdError;
