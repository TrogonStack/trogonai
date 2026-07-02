use chrono::Weekday;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DayOfWeekSetError {
    #[error("day_of_week set must not be empty")]
    Empty,
    #[error("day_of_week value {0} out of range 1..=7 (ISO weekday, 1=Monday)")]
    OutOfRange(i64),
}

/// A validated, non-empty set of ISO weekdays (`1..=7`, Monday = 1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DayOfWeekSet(Vec<Weekday>);

impl DayOfWeekSet {
    /// Construct from ISO weekday numbers (`1..=7`, Monday = 1), matching
    /// the dynamic-policy-conditions spec's `days_of_week` field.
    pub fn from_iso_numbers(days: impl IntoIterator<Item = i64>) -> Result<Self, DayOfWeekSetError> {
        let mut weekdays = Vec::new();
        for day in days {
            let iso: u32 = day
                .try_into()
                .ok()
                .filter(|n| (1..=7).contains(n))
                .ok_or(DayOfWeekSetError::OutOfRange(day))?;
            // `Weekday::try_from` treats `0` as Monday per the `chrono`
            // convention, so shift the 1-based ISO number down by one.
            let weekday = Weekday::try_from(u8::try_from(iso - 1).map_err(|_| DayOfWeekSetError::OutOfRange(day))?)
                .map_err(|_| DayOfWeekSetError::OutOfRange(day))?;
            weekdays.push(weekday);
        }
        if weekdays.is_empty() {
            return Err(DayOfWeekSetError::Empty);
        }
        Ok(Self(weekdays))
    }

    pub fn contains(&self, weekday: Weekday) -> bool {
        self.0.contains(&weekday)
    }
}
