#[derive(Debug)]
pub enum ScheduleSpecError {
    InvalidCronExpression {
        expr: String,
        source: Box<dyn std::error::Error>,
    },
    EverySecondsMustBePositive,
    EverySecondsTooLarge {
        max: u64,
        actual: u64,
    },
    InvalidHeaderName {
        name: String,
    },
    ReservedHeaderName {
        name: String,
    },
    InvalidHeaderValue {
        name: String,
    },
    InvalidRRuleDateTime {
        field: &'static str,
        value: String,
        source: Box<dyn std::error::Error>,
    },
    InvalidRRule {
        rrule: String,
        source: Box<dyn std::error::Error>,
    },
    InvalidRoute {
        route: String,
        source: trogon_nats::SubjectTokenViolation,
    },
    InvalidSamplingSource {
        subject: String,
        source: trogon_nats::SubjectTokenViolation,
    },
    InvalidTimezone {
        timezone: String,
    },
    InvalidTimezoneDatabaseVersion {
        version: String,
    },
    TtlMustBePositive,
    TtlSecondsTooLarge {
        max: u64,
        actual: u64,
    },
}

impl std::fmt::Display for ScheduleSpecError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidCronExpression { expr, source } => {
                write!(formatter, "cron expression '{expr}' is invalid: {source}")
            }
            Self::EverySecondsMustBePositive => formatter.write_str("every seconds must be positive"),
            Self::EverySecondsTooLarge { max, actual } => {
                write!(formatter, "every seconds must be at most {max}, got {actual}")
            }
            Self::InvalidHeaderName { name } => write!(formatter, "header name '{name}' is invalid"),
            Self::ReservedHeaderName { name } => write!(formatter, "header name '{name}' is reserved"),
            Self::InvalidHeaderValue { name } => write!(formatter, "header '{name}' contains an invalid value"),
            Self::InvalidRRuleDateTime { field, value, source } => {
                write!(formatter, "{field} datetime '{value}' is invalid: {source}")
            }
            Self::InvalidRRule { rrule, source } => write!(formatter, "rrule '{rrule}' is invalid: {source}"),
            Self::InvalidRoute { route, source } => write!(formatter, "delivery route '{route}' is invalid: {source}"),
            Self::InvalidSamplingSource { subject, source } => {
                write!(formatter, "sampling subject '{subject}' is invalid: {source}")
            }
            Self::InvalidTimezone { timezone } => write!(formatter, "timezone '{timezone}' is invalid"),
            Self::InvalidTimezoneDatabaseVersion { version } => {
                write!(formatter, "timezone database version '{version}' is invalid")
            }
            Self::TtlMustBePositive => formatter.write_str("ttl seconds must be positive"),
            Self::TtlSecondsTooLarge { max, actual } => {
                write!(formatter, "ttl seconds must be at most {max}, got {actual}")
            }
        }
    }
}

impl std::error::Error for ScheduleSpecError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidCronExpression { source, .. }
            | Self::InvalidRRuleDateTime { source, .. }
            | Self::InvalidRRule { source, .. } => Some(source.as_ref()),
            Self::InvalidRoute { source, .. } | Self::InvalidSamplingSource { source, .. } => Some(source),
            Self::EverySecondsMustBePositive
            | Self::EverySecondsTooLarge { .. }
            | Self::InvalidHeaderName { .. }
            | Self::ReservedHeaderName { .. }
            | Self::InvalidHeaderValue { .. }
            | Self::InvalidTimezone { .. }
            | Self::InvalidTimezoneDatabaseVersion { .. }
            | Self::TtlMustBePositive
            | Self::TtlSecondsTooLarge { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;

    #[test]
    fn display_covers_variants() {
        let source = || Box::new(std::io::Error::other("bad input")) as Box<dyn Error>;
        let invalid_route = trogon_nats::DottedNatsToken::new("bad*route").unwrap_err();
        let invalid_sampling = trogon_nats::DottedNatsToken::new("bad>subject").unwrap_err();

        let cases = [
            (
                ScheduleSpecError::InvalidCronExpression {
                    expr: "bad cron".to_string(),
                    source: source(),
                },
                "cron expression 'bad cron' is invalid: bad input",
                true,
            ),
            (
                ScheduleSpecError::EverySecondsMustBePositive,
                "every seconds must be positive",
                false,
            ),
            (
                ScheduleSpecError::EverySecondsTooLarge {
                    max: 315_576_000_000,
                    actual: 315_576_000_001,
                },
                "every seconds must be at most 315576000000, got 315576000001",
                false,
            ),
            (
                ScheduleSpecError::InvalidHeaderName {
                    name: "bad name".to_string(),
                },
                "header name 'bad name' is invalid",
                false,
            ),
            (
                ScheduleSpecError::ReservedHeaderName {
                    name: "Nats-Schedule".to_string(),
                },
                "header name 'Nats-Schedule' is reserved",
                false,
            ),
            (
                ScheduleSpecError::InvalidHeaderValue {
                    name: "x-kind".to_string(),
                },
                "header 'x-kind' contains an invalid value",
                false,
            ),
            (
                ScheduleSpecError::InvalidRRuleDateTime {
                    field: "dtstart",
                    value: "tomorrow".to_string(),
                    source: source(),
                },
                "dtstart datetime 'tomorrow' is invalid: bad input",
                true,
            ),
            (
                ScheduleSpecError::InvalidRRule {
                    rrule: "FREQ=NOPE".to_string(),
                    source: source(),
                },
                "rrule 'FREQ=NOPE' is invalid: bad input",
                true,
            ),
            (
                ScheduleSpecError::InvalidRoute {
                    route: "bad*route".to_string(),
                    source: invalid_route,
                },
                "delivery route 'bad*route' is invalid:",
                true,
            ),
            (
                ScheduleSpecError::InvalidSamplingSource {
                    subject: "bad>subject".to_string(),
                    source: invalid_sampling,
                },
                "sampling subject 'bad>subject' is invalid:",
                true,
            ),
            (
                ScheduleSpecError::InvalidTimezone {
                    timezone: "Nope/Zone".to_string(),
                },
                "timezone 'Nope/Zone' is invalid",
                false,
            ),
            (
                ScheduleSpecError::InvalidTimezoneDatabaseVersion {
                    version: "2025".to_string(),
                },
                "timezone database version '2025' is invalid",
                false,
            ),
            (
                ScheduleSpecError::TtlMustBePositive,
                "ttl seconds must be positive",
                false,
            ),
            (
                ScheduleSpecError::TtlSecondsTooLarge {
                    max: 315_576_000_000,
                    actual: 315_576_000_001,
                },
                "ttl seconds must be at most 315576000000, got 315576000001",
                false,
            ),
        ];

        for (error, expected, has_source) in cases {
            assert!(error.to_string().starts_with(expected), "{error}");
            assert_eq!(error.source().is_some(), has_source);
        }
    }
}
