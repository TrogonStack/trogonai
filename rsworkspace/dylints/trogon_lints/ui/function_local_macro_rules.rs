// edition:2024
#![allow(unused)]

// Module-level macro definitions are fine: must NOT be linted.
macro_rules! module_level {
    ($x:expr) => {
        $x + 1
    };
}

// Simple function-local `macro_rules!`: this is the violation.
fn route() -> u8 {
    macro_rules! local_add {
        ($x:expr) => {
            $x + 1
        };
    }
    local_add!(1)
}

// Nested inside a block within a function: still a violation.
fn nested() -> u8 {
    let value = {
        macro_rules! inner {
            () => {
                7
            };
        }
        inner!()
    };
    value
}

fn uses_module_level() -> u8 {
    module_level!(1)
}

fn main() {
    route();
    nested();
    uses_module_level();
}
