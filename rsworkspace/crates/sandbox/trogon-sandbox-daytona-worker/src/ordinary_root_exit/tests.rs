use super::*;

#[test]
fn exactly_one_of_a_code_and_a_signal_is_valid() {
    assert!(
        OrdinaryRootExit {
            exit_code: Some(0),
            signal: None
        }
        .is_valid()
    );
    assert!(
        OrdinaryRootExit {
            exit_code: None,
            signal: Some(9)
        }
        .is_valid()
    );
    assert!(
        !OrdinaryRootExit {
            exit_code: None,
            signal: None
        }
        .is_valid()
    );
    assert!(
        !OrdinaryRootExit {
            exit_code: Some(1),
            signal: Some(9)
        }
        .is_valid()
    );
}

#[test]
fn codes_and_signals_outside_their_ranges_are_invalid() {
    assert!(
        !OrdinaryRootExit {
            exit_code: Some(256),
            signal: None
        }
        .is_valid()
    );
    assert!(
        !OrdinaryRootExit {
            exit_code: Some(-1),
            signal: None
        }
        .is_valid()
    );
    assert!(
        !OrdinaryRootExit {
            exit_code: None,
            signal: Some(0)
        }
        .is_valid()
    );
    assert!(
        !OrdinaryRootExit {
            exit_code: None,
            signal: Some(65)
        }
        .is_valid()
    );
}

#[test]
fn a_signal_reports_as_a_shell_would() {
    assert_eq!(
        OrdinaryRootExit {
            exit_code: Some(3),
            signal: None
        }
        .code(),
        3
    );
    assert_eq!(
        OrdinaryRootExit {
            exit_code: None,
            signal: Some(15)
        }
        .code(),
        143
    );
}
