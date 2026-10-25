use std::time::Duration;

use trogon_std::NonZeroDuration;

use super::constants::ACK_LIMIT_MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AckBudget {
    limit: Duration,
    capped_from: Option<NonZeroDuration>,
}

impl AckBudget {
    pub fn new(requested: Option<NonZeroDuration>) -> Self {
        match requested {
            Some(requested) if Duration::from(requested) > ACK_LIMIT_MAX => Self {
                limit: ACK_LIMIT_MAX,
                capped_from: Some(requested),
            },
            Some(requested) => Self {
                limit: requested.into(),
                capped_from: None,
            },
            None => Self {
                limit: ACK_LIMIT_MAX,
                capped_from: None,
            },
        }
    }

    pub fn limit(self) -> Duration {
        self.limit
    }

    pub fn capped_from(self) -> Option<NonZeroDuration> {
        self.capped_from
    }
}

impl Default for AckBudget {
    fn default() -> Self {
        Self::new(None)
    }
}

#[cfg(test)]
mod tests;
