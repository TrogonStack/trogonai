use chrono::NaiveTime;
use serde::Deserialize;

use super::day_of_week_set::{DayOfWeekSet, DayOfWeekSetError};
use super::time_window::{TimeWindow, TimeWindowError};
use super::window_duration::{WindowDuration, WindowDurationError};
use crate::policy::tier2_dynamic::budget_metric::BudgetMetric;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Tier2DynamicConditionError {
    #[error(
        "dynamic_condition.type `{0}` is not recognized (expected time_window, day_of_week, token_count_per_window, or cost_per_window)"
    )]
    UnknownType(Box<str>),
    #[error("dynamic_condition of type `{type_name}` is missing required field `{field}`")]
    MissingField {
        type_name: &'static str,
        field: &'static str,
    },
    #[error("dynamic_condition.start_time or end_time `{0}` is not a valid HH:MM time")]
    InvalidTime(Box<str>),
    #[error(transparent)]
    DayOfWeek(#[from] DayOfWeekSetError),
    #[error(transparent)]
    TimeWindow(#[from] TimeWindowError),
    #[error(transparent)]
    WindowDuration(#[from] WindowDurationError),
}

/// A validated Tier-2 dynamic condition, attached to a `.cel` rule via its
/// optional `<rule>.dynamic.toml` sidecar. Evaluated with AND semantics
/// alongside the rule's CEL predicate: CEL must be true AND this
/// condition must be true for the rule to allow.
#[derive(Debug, Clone, PartialEq)]
pub enum Tier2DynamicCondition {
    TimeWindow(TimeWindow),
    DayOfWeek(DayOfWeekSet),
    TokenCountPerWindow { window: WindowDuration, limit: f64 },
    CostPerWindow { window: WindowDuration, limit: f64 },
}

impl Tier2DynamicCondition {
    pub fn budget_metric(&self) -> Option<BudgetMetric> {
        match self {
            Self::TokenCountPerWindow { .. } => Some(BudgetMetric::TokenCount),
            Self::CostPerWindow { .. } => Some(BudgetMetric::Cost),
            Self::TimeWindow(_) | Self::DayOfWeek(_) => None,
        }
    }
}

/// Raw TOML shape of a `<rule>.dynamic.toml` sidecar. Every field is
/// optional at this layer regardless of which `type` requires it --
/// requiredness is enforced per-variant in [`convert_dynamic_condition`]
/// so a missing field reports a field-aware error instead of a generic
/// TOML schema mismatch.
#[derive(Debug, Deserialize)]
pub(super) struct DynamicConditionToml {
    #[serde(rename = "type")]
    condition_type: String,
    timezone: Option<String>,
    start_time: Option<String>,
    end_time: Option<String>,
    days_of_week: Option<Vec<i64>>,
    window: Option<String>,
    limit: Option<f64>,
}

pub(super) fn convert_dynamic_condition(
    raw: DynamicConditionToml,
) -> Result<Tier2DynamicCondition, Tier2DynamicConditionError> {
    let timezone = raw.timezone.as_deref().unwrap_or("UTC");
    match raw.condition_type.as_str() {
        "time_window" => {
            let start_time = parse_time(field(raw.start_time.as_deref(), "time_window", "start_time")?)?;
            let end_time = parse_time(field(raw.end_time.as_deref(), "time_window", "end_time")?)?;
            let days = raw.days_of_week.map(DayOfWeekSet::from_iso_numbers).transpose()?;
            let window = TimeWindow::new(timezone, start_time, end_time, days)?;
            Ok(Tier2DynamicCondition::TimeWindow(window))
        }
        "day_of_week" => {
            let days_of_week = raw.days_of_week.ok_or(Tier2DynamicConditionError::MissingField {
                type_name: "day_of_week",
                field: "days_of_week",
            })?;
            let days = DayOfWeekSet::from_iso_numbers(days_of_week)?;
            Ok(Tier2DynamicCondition::DayOfWeek(days))
        }
        "token_count_per_window" => {
            let window = WindowDuration::parse(field(raw.window.as_deref(), "token_count_per_window", "window")?)?;
            let limit = raw.limit.ok_or(Tier2DynamicConditionError::MissingField {
                type_name: "token_count_per_window",
                field: "limit",
            })?;
            Ok(Tier2DynamicCondition::TokenCountPerWindow { window, limit })
        }
        "cost_per_window" => {
            let window = WindowDuration::parse(field(raw.window.as_deref(), "cost_per_window", "window")?)?;
            let limit = raw.limit.ok_or(Tier2DynamicConditionError::MissingField {
                type_name: "cost_per_window",
                field: "limit",
            })?;
            Ok(Tier2DynamicCondition::CostPerWindow { window, limit })
        }
        other => Err(Tier2DynamicConditionError::UnknownType(other.into())),
    }
}

fn field<'a>(
    value: Option<&'a str>,
    type_name: &'static str,
    field: &'static str,
) -> Result<&'a str, Tier2DynamicConditionError> {
    value.ok_or(Tier2DynamicConditionError::MissingField { type_name, field })
}

fn parse_time(raw: &str) -> Result<NaiveTime, Tier2DynamicConditionError> {
    NaiveTime::parse_from_str(raw, "%H:%M").map_err(|_| Tier2DynamicConditionError::InvalidTime(raw.into()))
}
