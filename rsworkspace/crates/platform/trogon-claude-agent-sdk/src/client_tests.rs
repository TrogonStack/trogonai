use super::connection_failure;
use crate::{ControlOperation, Error};

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
