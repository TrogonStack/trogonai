use super::*;
use crate::{Limits, Timeout};
use std::{collections::BTreeMap, ffi::OsString};

struct Variables(BTreeMap<OsString, OsString>);

impl EnvironmentVariable for Variables {
    fn var_os(&self, key: &OsStr) -> Option<OsString> {
        self.0.get(key).cloned()
    }
}

fn process(value: Option<&str>) -> Variables {
    Variables(
        value
            .map(|value| (OsString::from(VARIABLE), OsString::from(value)))
            .into_iter()
            .collect(),
    )
}

fn options(limits: Limits, mode: EnvironmentMode, configured: Option<&str>) -> ClaudeAgentOptions {
    let mut options = ClaudeAgentOptions {
        limits,
        environment_mode: mode,
        ..ClaudeAgentOptions::default()
    };
    if let Some(value) = configured {
        options.env.insert(VARIABLE.into(), value.into());
    }
    options
}

fn millis(value: u64) -> Option<Duration> {
    Some(Duration::from_millis(value))
}

#[test]
fn typescript_without_options_env_reads_the_process_environment() {
    let options = options(Limits::default(), EnvironmentMode::TypeScript, None);
    assert_eq!(resolve(&options, &process(None)), millis(600_000));
    assert_eq!(resolve(&options, &process(Some("1e3"))), millis(1000));
    assert_eq!(resolve(&options, &process(Some("0"))), None);
}

#[test]
fn typescript_with_options_env_reads_only_options_env() {
    let options = options(Limits::default(), EnvironmentMode::TypeScript, Some("2_500"));
    assert_eq!(resolve(&options, &process(Some("7"))), millis(2500));
    let mut other = options.clone();
    other.env.insert("OTHER".into(), "1".into());
    other.env.remove(VARIABLE);
    assert_eq!(resolve(&other, &process(Some("7"))), millis(600_000));
}

#[test]
fn an_explicit_environment_mode_picks_where_typescript_reads() {
    let replace = options(Limits::default(), EnvironmentMode::Replace, None);
    assert_eq!(resolve(&replace, &process(Some("7"))), millis(600_000));
    let inherit = options(Limits::default(), EnvironmentMode::Inherit, Some("5"));
    assert_eq!(resolve(&inherit, &process(Some("7"))), millis(7));
    let overlay = options(Limits::default(), EnvironmentMode::Overlay, Some("5"));
    assert_eq!(resolve(&overlay, &process(Some("7"))), millis(5));
    let overlay = options(Limits::default(), EnvironmentMode::Overlay, None);
    assert_eq!(resolve(&overlay, &process(Some("7"))), millis(7));
}

#[test]
fn python_reads_options_env_then_the_process_environment() {
    let both = options(Limits::python(), EnvironmentMode::TypeScript, Some("5"));
    assert_eq!(resolve(&both, &process(Some("7"))), millis(5));
    let neither = options(Limits::python(), EnvironmentMode::TypeScript, None);
    assert_eq!(resolve(&neither, &process(None)), millis(600_000));
    assert_eq!(resolve(&neither, &process(Some("7"))), millis(7));
    assert_eq!(resolve(&neither, &process(Some("1e3"))), millis(600_000));
    // A set but empty value in `options.env` wins over the process environment and is not an integer.
    let empty = options(Limits::python(), EnvironmentMode::TypeScript, Some(""));
    assert_eq!(resolve(&empty, &process(Some("7"))), millis(600_000));
}

#[test]
fn a_fixed_ceiling_ignores_the_environment() {
    let limits = Limits {
        run_end_timeout: RunEndTimeout::Fixed(Timeout::new(Duration::from_millis(300))),
        ..Limits::default()
    };
    let options = options(limits, EnvironmentMode::TypeScript, Some("0"));
    assert_eq!(resolve(&options, &process(Some("7"))), millis(300));
    assert_eq!(
        RunEndTimeout::Fixed(Timeout::new(Duration::ZERO)).for_variable(None),
        millis(0)
    );
}

#[test]
fn the_parse_helpers_agree_on_their_edges() {
    assert!(!is_exponent_form("1e"));
    assert!(!is_exponent_form(".e1"));
    assert!(!is_exponent_form("."));
    assert!(is_exponent_form("+.5E-1"));
    assert_eq!(grouped_digits("-1 000"), Some(-1000.0));
    assert_eq!(grouped_digits("1 00"), None);
    assert_eq!(grouped_digits("1,000_000"), None);
    assert_eq!(grouped_digits("1000"), None);
    assert_eq!(decimal_digit('\u{1d7ff}'), Some(9));
    assert_eq!(decimal_digit('\u{1d800}'), None);
    assert_eq!(decimal_digit('\u{0}'), None);
    assert_eq!(python_int("-0"), Some(0));
    assert_eq!(python_int("-1"), None);
    assert_eq!(python_int(&"9".repeat(4300)), Some(u64::MAX));
    assert_eq!(python_int(&"9".repeat(4301)), None);
}
