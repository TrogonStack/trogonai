use chrono::Duration;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WindowDurationError {
    #[error("window duration must not be empty")]
    Empty,
    #[error("window duration `{0}` missing unit suffix (expected m, h, or d)")]
    MissingUnit(Box<str>),
    #[error("window duration `{0}` has unknown unit (expected m, h, or d)")]
    UnknownUnit(Box<str>),
    #[error("window duration `{0}` integer part must be a positive number")]
    InvalidAmount(Box<str>),
}

/// A budget-window duration parsed from the spec's `<int><unit>` format,
/// unit in `m` (minutes), `h` (hours), or `d` (days).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowDuration(Duration);

impl WindowDuration {
    pub fn parse(raw: &str) -> Result<Self, WindowDurationError> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Err(WindowDurationError::Empty);
        }
        let Some((amount, unit)) = split_amount_and_unit(trimmed) else {
            return Err(WindowDurationError::MissingUnit(trimmed.into()));
        };
        let amount: i64 = amount
            .parse()
            .ok()
            .filter(|n| *n > 0)
            .ok_or_else(|| WindowDurationError::InvalidAmount(trimmed.into()))?;
        let duration = match unit {
            "m" => Duration::minutes(amount),
            "h" => Duration::hours(amount),
            "d" => Duration::days(amount),
            _ => return Err(WindowDurationError::UnknownUnit(trimmed.into())),
        };
        Ok(Self(duration))
    }

    pub fn as_chrono_duration(&self) -> Duration {
        self.0
    }

    pub fn as_seconds(&self) -> i64 {
        self.0.num_seconds()
    }
}

fn split_amount_and_unit(raw: &str) -> Option<(&str, &str)> {
    let split_at = raw.len().checked_sub(1)?;
    Some(raw.split_at(split_at))
}
