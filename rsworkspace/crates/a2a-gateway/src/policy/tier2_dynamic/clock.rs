use chrono::{DateTime, Utc};

/// Injectable wall-clock source for Tier-2 dynamic conditions
/// (`time_window`, `day_of_week`, and window-bucket rollover for
/// `token_count_per_window` / `cost_per_window`). Mirrors
/// [`crate::policy::tier1_declarative::evaluator::Tier1Clock`], but yields
/// a `chrono::DateTime<Utc>` since dynamic conditions need calendar-aware
/// (timezone, day-of-week) reasoning that `std::time::SystemTime` can't
/// express on its own.
pub trait Tier2Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

#[derive(Debug, Default)]
pub struct SystemTier2Clock;

impl Tier2Clock for SystemTier2Clock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

#[derive(Debug, Clone)]
pub struct FixedTier2Clock(DateTime<Utc>);

impl FixedTier2Clock {
    pub fn new(instant: DateTime<Utc>) -> Self {
        Self(instant)
    }
}

impl Tier2Clock for FixedTier2Clock {
    fn now(&self) -> DateTime<Utc> {
        self.0
    }
}
