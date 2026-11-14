#![cfg_attr(
    dylint_lib = "trogon_dylint_lints",
    allow(serde_json_macro, reason = "Tests exercise exact upstream JSON values.")
)]

use serde_json::json;

use super::{claim_initialization, connection_failure};
use crate::{ControlOperation, Error, JsRuntime};

#[test]
fn startup_error_is_preserved_when_cleanup_succeeds_or_only_loses_the_driver() {
    for cleanup in [Ok(()), Err(Error::DriverStopped)] {
        assert!(matches!(
            connection_failure(Error::TooManyControls, cleanup),
            Error::TooManyControls
        ));
    }
    assert!(matches!(
        connection_failure(Error::DriverStopped, Ok(())),
        Error::DriverStopped
    ));
    assert!(matches!(
        connection_failure(Error::DriverStopped, Err(Error::UnexpectedEof)),
        Error::UnexpectedEof
    ));
}

#[test]
fn identical_startup_policy_failures_are_not_reported_twice() {
    assert!(matches!(
        connection_failure(Error::BlockingHooksUnsupported, Err(Error::BlockingHooksUnsupported)),
        Error::BlockingHooksUnsupported
    ));
    assert!(matches!(
        connection_failure(Error::HooksNotApplied, Err(Error::HooksNotApplied)),
        Error::HooksNotApplied
    ));
    assert!(matches!(
        connection_failure(Error::ManagedSettingsNotApplied, Err(Error::ManagedSettingsNotApplied)),
        Error::ManagedSettingsNotApplied
    ));
    assert!(matches!(
        connection_failure(
            Error::ControlTimeout {
                operation: ControlOperation::Initialize
            },
            Err(Error::ControlTimeout {
                operation: ControlOperation::Initialize
            })
        ),
        Error::ControlTimeout {
            operation: ControlOperation::Initialize
        }
    ));
}

#[test]
fn distinct_startup_and_cleanup_failures_retain_both_typed_causes() {
    let error = connection_failure(
        Error::ControlTimeout {
            operation: ControlOperation::Initialize,
        },
        Err(Error::ControlTimeout {
            operation: ControlOperation::Interrupt,
        }),
    );
    assert!(matches!(error, Error::Cleanup { primary, cleanup }
        if matches!(*primary, Error::ControlTimeout { operation: ControlOperation::Initialize })
        && matches!(*cleanup, Error::ControlTimeout { operation: ControlOperation::Interrupt })));
    assert!(matches!(
        connection_failure(Error::BlockingHooksUnsupported, Err(Error::HooksNotApplied)),
        Error::Cleanup { .. }
    ));
}

#[test]
fn a_claim_over_a_failed_or_unsettled_result_keeps_no_flags() -> Result<(), Box<dyn std::error::Error>> {
    let failed = Err(std::sync::Arc::new(Error::DriverStopped));
    let update = json!({"commands":[],"hooks_applied":null});
    for previous in [Some(&failed), None] {
        let merged = claim_initialization(previous, &update, JsRuntime::Node).map_err(|error| error.to_string())?;
        assert_eq!(merged, Some(update.clone()));
    }
    Ok(())
}

#[test]
fn a_claim_initialize_string_spreads_its_utf16_code_units() -> Result<(), Box<dyn std::error::Error>> {
    let previous = Ok(Some(json!({"hooks_applied":false})));
    let merged = claim_initialization(Some(&previous), &json!("a\u{1F600}"), JsRuntime::Bun)
        .map_err(|error| error.to_string())?;
    assert_eq!(
        merged,
        Some(json!({"0":"a","1":"\u{FFFD}","2":"\u{FFFD}","hooks_applied":false}))
    );
    Ok(())
}
