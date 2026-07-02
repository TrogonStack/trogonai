use chrono::{DateTime, NaiveDate, NaiveTime, Utc};

use super::budget_metric::BudgetMetric;
use super::clock::{FixedTier2Clock, SystemTier2Clock, Tier2Clock};
use super::day_of_week_set::{DayOfWeekSet, DayOfWeekSetError};
use super::dynamic_condition::{Tier2DynamicCondition, convert_dynamic_condition};
use super::sidecar::{Tier2DynamicSidecarError, load_dynamic_condition_sidecar};
use super::time_window::{TimeWindow, TimeWindowError};
use super::window_duration::{WindowDuration, WindowDurationError};
use super::windowed_budget::WindowedBudget;
use crate::policy::tier2::rule_name::RuleName;

fn utc(y: i32, m: u32, d: u32, h: u32, min: u32) -> DateTime<Utc> {
    NaiveDate::from_ymd_opt(y, m, d)
        .and_then(|date| date.and_hms_opt(h, min, 0))
        .expect("valid test datetime")
        .and_utc()
}

fn time(h: u32, m: u32) -> NaiveTime {
    NaiveTime::from_hms_opt(h, m, 0).expect("valid test time")
}

// --- DayOfWeekSet ---

#[test]
fn day_of_week_set_from_iso_numbers_accepts_full_range() {
    let set = DayOfWeekSet::from_iso_numbers([1, 7]).expect("valid iso days");
    assert!(set.contains(chrono::Weekday::Mon));
    assert!(set.contains(chrono::Weekday::Sun));
    assert!(!set.contains(chrono::Weekday::Wed));
}

#[test]
fn day_of_week_set_rejects_empty() {
    assert_eq!(
        DayOfWeekSet::from_iso_numbers([]).unwrap_err(),
        DayOfWeekSetError::Empty
    );
}

#[test]
fn day_of_week_set_rejects_out_of_range() {
    assert_eq!(
        DayOfWeekSet::from_iso_numbers([0]).unwrap_err(),
        DayOfWeekSetError::OutOfRange(0)
    );
    assert_eq!(
        DayOfWeekSet::from_iso_numbers([8]).unwrap_err(),
        DayOfWeekSetError::OutOfRange(8)
    );
}

// --- WindowDuration ---

#[test]
fn window_duration_parses_minutes_hours_days() {
    assert_eq!(WindowDuration::parse("30m").expect("30m").as_seconds(), 1800);
    assert_eq!(WindowDuration::parse("2h").expect("2h").as_seconds(), 7200);
    assert_eq!(WindowDuration::parse("1d").expect("1d").as_seconds(), 86_400);
}

#[test]
fn window_duration_rejects_empty() {
    assert_eq!(WindowDuration::parse("").unwrap_err(), WindowDurationError::Empty);
    assert_eq!(WindowDuration::parse("   ").unwrap_err(), WindowDurationError::Empty);
}

#[test]
fn window_duration_rejects_unknown_unit() {
    assert!(matches!(
        WindowDuration::parse("5x").unwrap_err(),
        WindowDurationError::UnknownUnit(_)
    ));
}

#[test]
fn window_duration_rejects_non_positive_amount() {
    assert!(matches!(
        WindowDuration::parse("0m").unwrap_err(),
        WindowDurationError::InvalidAmount(_)
    ));
    assert!(matches!(
        WindowDuration::parse("-1h").unwrap_err(),
        WindowDurationError::InvalidAmount(_)
    ));
}

// --- TimeWindow ---

#[test]
fn time_window_same_day_interval_is_start_inclusive_end_exclusive() {
    let window = TimeWindow::new("UTC", time(9, 0), time(17, 0), None).expect("valid window");
    assert!(window.contains(utc(2026, 7, 2, 9, 0)));
    assert!(window.contains(utc(2026, 7, 2, 16, 59)));
    assert!(!window.contains(utc(2026, 7, 2, 17, 0)));
    assert!(!window.contains(utc(2026, 7, 2, 8, 59)));
}

#[test]
fn time_window_wraps_past_midnight() {
    let window = TimeWindow::new("UTC", time(22, 0), time(6, 0), None).expect("valid window");
    assert!(window.contains(utc(2026, 7, 2, 23, 0)));
    assert!(window.contains(utc(2026, 7, 2, 0, 30)));
    assert!(!window.contains(utc(2026, 7, 2, 12, 0)));
}

#[test]
fn time_window_restricted_to_days_of_week() {
    let days = DayOfWeekSet::from_iso_numbers([6, 7]).expect("weekend");
    let window = TimeWindow::new("UTC", time(0, 0), time(23, 59), Some(days)).expect("valid window");
    // 2026-07-04 is a Saturday.
    assert!(window.contains(utc(2026, 7, 4, 12, 0)));
    // 2026-07-02 is a Thursday.
    assert!(!window.contains(utc(2026, 7, 2, 12, 0)));
}

#[test]
fn time_window_converts_to_configured_timezone() {
    // America/New_York is UTC-4 in July (EDT). 13:30 UTC is 09:30 local.
    let window = TimeWindow::new("America/New_York", time(9, 0), time(17, 0), None).expect("valid window");
    assert!(window.contains(utc(2026, 7, 2, 13, 30)));
    assert!(!window.contains(utc(2026, 7, 2, 12, 30)));
}

#[test]
fn time_window_rejects_unknown_timezone() {
    assert!(matches!(
        TimeWindow::new("Not/AZone", time(0, 0), time(1, 0), None).unwrap_err(),
        TimeWindowError::UnknownTimezone(_)
    ));
}

// --- Clock ---

#[test]
fn fixed_tier2_clock_returns_configured_instant() {
    let instant = utc(2026, 7, 2, 10, 0);
    let clock = FixedTier2Clock::new(instant);
    assert_eq!(clock.now(), instant);
}

#[test]
fn system_tier2_clock_returns_recent_time() {
    let before = Utc::now();
    let now = SystemTier2Clock.now();
    assert!(now >= before);
}

// --- WindowedBudget ---

#[test]
fn windowed_budget_allows_while_within_limit() {
    let budget = WindowedBudget::new();
    let rule = RuleName::new("budget-rule").expect("non-empty test rule name");
    let window = WindowDuration::parse("1h").expect("1h");
    let now = utc(2026, 7, 2, 10, 0);
    assert!(budget.check_and_record(&rule, BudgetMetric::TokenCount, window, 100.0, 40.0, now));
    assert!(budget.check_and_record(&rule, BudgetMetric::TokenCount, window, 100.0, 40.0, now));
}

#[test]
fn windowed_budget_denies_once_limit_exceeded() {
    let budget = WindowedBudget::new();
    let rule = RuleName::new("budget-rule").expect("non-empty test rule name");
    let window = WindowDuration::parse("1h").expect("1h");
    let now = utc(2026, 7, 2, 10, 0);
    assert!(budget.check_and_record(&rule, BudgetMetric::TokenCount, window, 100.0, 60.0, now));
    assert!(!budget.check_and_record(&rule, BudgetMetric::TokenCount, window, 100.0, 60.0, now));
}

#[test]
fn windowed_budget_at_exact_limit_passes() {
    let budget = WindowedBudget::new();
    let rule = RuleName::new("budget-rule").expect("non-empty test rule name");
    let window = WindowDuration::parse("1h").expect("1h");
    let now = utc(2026, 7, 2, 10, 0);
    assert!(budget.check_and_record(&rule, BudgetMetric::Cost, window, 10.0, 10.0, now));
}

#[test]
fn windowed_budget_resets_on_window_rollover() {
    let budget = WindowedBudget::new();
    let rule = RuleName::new("budget-rule").expect("non-empty test rule name");
    let window = WindowDuration::parse("1h").expect("1h");
    let first_window = utc(2026, 7, 2, 10, 30);
    let next_window = utc(2026, 7, 2, 11, 30);
    assert!(budget.check_and_record(&rule, BudgetMetric::TokenCount, window, 100.0, 100.0, first_window));
    assert!(!budget.check_and_record(&rule, BudgetMetric::TokenCount, window, 100.0, 1.0, first_window));
    assert!(budget.check_and_record(&rule, BudgetMetric::TokenCount, window, 100.0, 1.0, next_window));
}

#[test]
fn windowed_budget_tracks_metrics_independently() {
    let budget = WindowedBudget::new();
    let rule = RuleName::new("budget-rule").expect("non-empty test rule name");
    let window = WindowDuration::parse("1h").expect("1h");
    let now = utc(2026, 7, 2, 10, 0);
    assert!(budget.check_and_record(&rule, BudgetMetric::TokenCount, window, 10.0, 10.0, now));
    // Cost metric on the same rule has its own independent budget.
    assert!(budget.check_and_record(&rule, BudgetMetric::Cost, window, 10.0, 10.0, now));
}

#[test]
fn windowed_budget_tracks_rules_independently() {
    let budget = WindowedBudget::new();
    let rule_a = RuleName::new("rule-a").expect("non-empty test rule name");
    let rule_b = RuleName::new("rule-b").expect("non-empty test rule name");
    let window = WindowDuration::parse("1h").expect("1h");
    let now = utc(2026, 7, 2, 10, 0);
    assert!(budget.check_and_record(&rule_a, BudgetMetric::TokenCount, window, 10.0, 10.0, now));
    assert!(budget.check_and_record(&rule_b, BudgetMetric::TokenCount, window, 10.0, 10.0, now));
}

// --- Tier2DynamicCondition TOML wire conversion ---

#[test]
fn convert_dynamic_condition_parses_time_window() {
    let toml_src = r#"
        type = "time_window"
        timezone = "UTC"
        start_time = "09:00"
        end_time = "17:00"
        days_of_week = [1, 2, 3, 4, 5]
    "#;
    let raw = toml::from_str(toml_src).expect("valid toml");
    let condition = convert_dynamic_condition(raw).expect("valid condition");
    assert!(matches!(condition, Tier2DynamicCondition::TimeWindow(_)));
}

#[test]
fn convert_dynamic_condition_parses_day_of_week() {
    let toml_src = r#"
        type = "day_of_week"
        days_of_week = [6, 7]
    "#;
    let raw = toml::from_str(toml_src).expect("valid toml");
    let condition = convert_dynamic_condition(raw).expect("valid condition");
    assert!(matches!(condition, Tier2DynamicCondition::DayOfWeek(_)));
}

#[test]
fn convert_dynamic_condition_parses_token_count_per_window() {
    let toml_src = r#"
        type = "token_count_per_window"
        window = "1h"
        limit = 1000
    "#;
    let raw = toml::from_str(toml_src).expect("valid toml");
    let condition = convert_dynamic_condition(raw).expect("valid condition");
    assert!(matches!(
        condition,
        Tier2DynamicCondition::TokenCountPerWindow { limit, .. } if limit == 1000.0
    ));
}

#[test]
fn convert_dynamic_condition_parses_cost_per_window() {
    let toml_src = r#"
        type = "cost_per_window"
        window = "1d"
        limit = 50.5
    "#;
    let raw = toml::from_str(toml_src).expect("valid toml");
    let condition = convert_dynamic_condition(raw).expect("valid condition");
    assert!(matches!(
        condition,
        Tier2DynamicCondition::CostPerWindow { limit, .. } if limit == 50.5
    ));
}

#[test]
fn convert_dynamic_condition_rejects_unknown_type() {
    let toml_src = r#"type = "geo_fence""#;
    let raw = toml::from_str(toml_src).expect("valid toml");
    assert!(convert_dynamic_condition(raw).is_err());
}

#[test]
fn convert_dynamic_condition_reports_missing_field() {
    let toml_src = r#"type = "time_window""#;
    let raw = toml::from_str(toml_src).expect("valid toml");
    assert!(convert_dynamic_condition(raw).is_err());
}

// --- sidecar loader ---

#[test]
fn sidecar_loader_returns_none_when_absent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cel_path = dir.path().join("rule.cel");
    std::fs::write(&cel_path, "true").expect("write cel");
    let result = load_dynamic_condition_sidecar(&cel_path).expect("no sidecar is not an error");
    assert!(result.is_none());
}

#[test]
fn sidecar_loader_loads_valid_sidecar() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cel_path = dir.path().join("rule.cel");
    std::fs::write(&cel_path, "true").expect("write cel");
    std::fs::write(
        dir.path().join("rule.dynamic.toml"),
        "type = \"day_of_week\"\ndays_of_week = [1, 2, 3, 4, 5]\n",
    )
    .expect("write sidecar");
    let result = load_dynamic_condition_sidecar(&cel_path).expect("valid sidecar");
    assert!(matches!(result, Some(Tier2DynamicCondition::DayOfWeek(_))));
}

#[test]
fn sidecar_loader_fails_closed_on_malformed_toml() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cel_path = dir.path().join("rule.cel");
    std::fs::write(&cel_path, "true").expect("write cel");
    std::fs::write(dir.path().join("rule.dynamic.toml"), "not valid toml [[[").expect("write sidecar");
    let err = load_dynamic_condition_sidecar(&cel_path).expect_err("malformed toml rejected");
    assert!(matches!(err, Tier2DynamicSidecarError::ParseToml { .. }));
}

#[test]
fn sidecar_loader_fails_closed_on_schema_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let cel_path = dir.path().join("rule.cel");
    std::fs::write(&cel_path, "true").expect("write cel");
    std::fs::write(dir.path().join("rule.dynamic.toml"), "type = \"day_of_week\"\n").expect("write sidecar");
    let err = load_dynamic_condition_sidecar(&cel_path).expect_err("missing days_of_week rejected");
    assert!(matches!(err, Tier2DynamicSidecarError::Schema { .. }));
}
