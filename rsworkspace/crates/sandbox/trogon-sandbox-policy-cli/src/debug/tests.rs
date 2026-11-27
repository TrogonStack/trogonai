use std::collections::HashMap;
use std::env::VarError;

use super::*;

struct StubEnv(HashMap<&'static str, &'static str>);

impl ReadEnv for StubEnv {
    fn var(&self, key: &str) -> Result<String, VarError> {
        self.0
            .get(key)
            .map(|value| value.to_string())
            .ok_or(VarError::NotPresent)
    }
}

#[test]
fn any_non_empty_debug_value_turns_debugging_on() {
    assert!(Debug::enabled_by(Some("1")));
    assert!(Debug::enabled_by(Some("0")));
    assert!(Debug::enabled_by(Some("false")));
    assert!(!Debug::enabled_by(Some("")));
    assert!(!Debug::enabled_by(None));
}

#[test]
fn the_environment_variable_turns_debugging_on_without_the_flag() {
    let args = Args::default();
    let set = StubEnv(HashMap::from([("TROGON_SANDBOX_POLICY_DEBUG", "1")]));
    let unset = StubEnv(HashMap::new());
    assert!(Debug::from_env(&args, &set).0);
    assert!(!Debug::from_env(&args, &unset).0);
}

#[test]
fn the_flag_turns_debugging_on_without_the_environment_variable() {
    let args = Args {
        debug: true,
        ..Args::default()
    };
    assert!(Debug::from_env(&args, &StubEnv(HashMap::new())).0);
}
