use std::str::FromStr;

use chrono::{DateTime, Utc};

use crate::commands::domain::{RRuleTimezone, Schedule};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RRuleCursor {
    AtOrAfter(DateTime<Utc>),
    After(DateTime<Utc>),
}

impl RRuleCursor {
    pub(crate) fn at_or_after(instant: DateTime<Utc>) -> Self {
        Self::AtOrAfter(instant)
    }

    pub(crate) fn after(instant: DateTime<Utc>) -> Self {
        Self::After(instant)
    }

    fn instant(self) -> DateTime<Utc> {
        match self {
            Self::AtOrAfter(instant) | Self::After(instant) => instant,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum RRuleExpansionError {
    #[error("schedule is not an RRULE schedule")]
    NotRRule,
    #[error("RRULE set could not be expanded: {source}")]
    Expansion {
        #[source]
        source: rrule::RRuleError,
    },
}

pub(crate) fn next_rrule_occurrence(
    schedule: &Schedule,
    cursor: RRuleCursor,
) -> Result<Option<DateTime<Utc>>, RRuleExpansionError> {
    let Schedule::RRule {
        dtstart,
        rrule,
        timezone,
        rdate,
        exdate,
    } = schedule
    else {
        return Err(RRuleExpansionError::NotRRule);
    };

    let tz = rrule_timezone(timezone.as_ref());
    let dtstart = dtstart.to_datetime().with_timezone(&tz);
    let mut set = rrule::RRuleSet::new(dtstart)
        .set_from_string(&format!("RRULE:{}", rrule.as_str()))
        .map_err(|source| RRuleExpansionError::Expansion { source })?;

    for date in rdate {
        set = set.rdate(date.to_datetime().with_timezone(&tz));
    }
    for date in exdate {
        set = set.exdate(date.to_datetime().with_timezone(&tz));
    }

    let instant = cursor.instant();
    let candidates = set.after(instant.with_timezone(&tz)).all(2).dates;
    let occurrence = match cursor {
        RRuleCursor::AtOrAfter(_) => candidates.into_iter().next(),
        RRuleCursor::After(_) => candidates.into_iter().find(|date| date.with_timezone(&Utc) > instant),
    };

    Ok(occurrence.map(|date| date.with_timezone(&Utc)))
}

fn rrule_timezone(timezone: Option<&RRuleTimezone>) -> rrule::Tz {
    timezone
        .map(|timezone| {
            chrono_tz::Tz::from_str(timezone.as_str())
                .expect("RRuleTimezone validates IANA timezone ids")
                .into()
        })
        .unwrap_or(rrule::Tz::UTC)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::domain::{RRuleDateTime, RRuleExpression, RRuleTimezone};

    fn instant(raw: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(raw).unwrap().with_timezone(&Utc)
    }

    #[test]
    fn daily_rrule_returns_the_next_occurrence_at_or_after_the_cursor() {
        let schedule = Schedule::rrule("2026-01-01T00:00:00Z", "FREQ=DAILY;COUNT=3", None).unwrap();

        let next = next_rrule_occurrence(&schedule, RRuleCursor::at_or_after(instant("2026-01-01T12:00:00Z"))).unwrap();

        assert_eq!(next, Some(instant("2026-01-02T00:00:00Z")));
    }

    #[test]
    fn strict_after_cursor_does_not_repeat_the_fired_occurrence() {
        let schedule = Schedule::rrule("2026-01-01T00:00:00Z", "FREQ=DAILY;COUNT=2", None).unwrap();

        let next = next_rrule_occurrence(&schedule, RRuleCursor::after(instant("2026-01-01T00:00:00Z"))).unwrap();

        assert_eq!(next, Some(instant("2026-01-02T00:00:00Z")));
    }

    #[test]
    fn rdate_and_exdate_adjust_the_next_occurrence() {
        let schedule = Schedule::RRule {
            dtstart: RRuleDateTime::new("dtstart", "2026-01-01T00:00:00Z").unwrap(),
            rrule: RRuleExpression::new("FREQ=DAILY;COUNT=2").unwrap(),
            timezone: None,
            rdate: vec![RRuleDateTime::new("rdate", "2026-01-04T00:00:00Z").unwrap()],
            exdate: vec![RRuleDateTime::new("exdate", "2026-01-02T00:00:00Z").unwrap()],
        };

        let next = next_rrule_occurrence(&schedule, RRuleCursor::after(instant("2026-01-01T00:00:00Z"))).unwrap();

        assert_eq!(next, Some(instant("2026-01-04T00:00:00Z")));
    }

    #[test]
    fn timezone_expansion_preserves_wall_clock_across_dst() {
        let schedule = Schedule::RRule {
            dtstart: RRuleDateTime::new("dtstart", "2026-03-07T14:00:00Z").unwrap(),
            rrule: RRuleExpression::new("FREQ=DAILY;COUNT=3").unwrap(),
            timezone: Some(RRuleTimezone::new("America/New_York").unwrap()),
            rdate: Vec::new(),
            exdate: Vec::new(),
        };

        let next = next_rrule_occurrence(&schedule, RRuleCursor::at_or_after(instant("2026-03-08T12:00:00Z"))).unwrap();

        assert_eq!(next, Some(instant("2026-03-08T13:00:00Z")));
    }

    #[test]
    fn non_rrule_schedules_are_rejected() {
        let schedule = Schedule::every(std::time::Duration::from_secs(30)).unwrap();

        let error =
            next_rrule_occurrence(&schedule, RRuleCursor::at_or_after(instant("2026-01-01T00:00:00Z"))).unwrap_err();

        assert!(matches!(error, RRuleExpansionError::NotRRule));
    }
}
