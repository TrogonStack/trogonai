use super::*;

#[test]
fn instantiation_that_trapped_is_reported_as_a_trap() {
    let error = SimError::Instantiate {
        source: wasmtime::Error::new(wasmtime::Trap::OutOfFuel),
    };

    assert!(error.is_trap());
}

#[test]
fn instantiation_that_failed_for_another_reason_is_not_a_trap() {
    let error = SimError::Instantiate {
        source: wasmtime::Error::msg("missing export"),
    };

    assert!(!error.is_trap());
}

#[test]
fn failures_outside_instantiation_are_never_traps() {
    let compile = SimError::Compile {
        source: wasmtime::Error::new(wasmtime::Trap::OutOfFuel),
    };
    let arm = SimError::Arm {
        source: wasmtime::Error::new(wasmtime::Trap::OutOfFuel),
    };

    assert!(!compile.is_trap());
    assert!(!arm.is_trap());
}
