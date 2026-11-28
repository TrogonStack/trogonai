//! How `network.requireHostMatch` reads and where it reaches.

use super::*;

#[test]
fn require_host_match_reaches_the_network_options() -> TestResult {
    assert_eq!(
        network_options_of(r#""requireHostMatch": false"#)?.host_match,
        HostMatch::NotRequired
    );
    assert_eq!(
        network_options_of(r#""requireHostMatch": true"#)?.host_match,
        HostMatch::Required
    );
    assert_eq!(
        network_options_of(r#""allowPlaintextHeaderSet": false"#)?.host_match,
        HostMatch::Required
    );
    Ok(())
}

#[test]
fn require_host_match_false_turns_matching_off_without_a_warning() -> TestResult {
    let settings = accepted(&with_network_extra("requireHostMatch", "false"))?;
    assert!(settings.warnings().is_empty(), "{:?}", settings.warnings());
    assert_eq!(
        accepted(&with_network_extra_pairs(r#""requireHostMatch": false"#))?.host_match(),
        Some(HostMatch::NotRequired)
    );
    assert_eq!(
        accepted(&with_network_extra_pairs(r#""requireHostMatch": true"#))?.host_match(),
        Some(HostMatch::Required)
    );
    assert_eq!(
        accepted(&with_network_extra_pairs(r#""allowPlaintextHeaderSet": false"#))?.host_match(),
        None
    );
    Ok(())
}
