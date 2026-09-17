// edition:2024
#![allow(dead_code, constant_outside_constants_module)]

use std::marker::PhantomData;

#[path = "auxiliary/fixed_literal_generated.rs"]
mod generated;

const VERSION_CURRENT: &str = "current";
const VERSION_PREVIOUS: &str = "previous";

const _: () = assert!(!VERSION_CURRENT.is_empty());
const _: () = assert!(!VERSION_PREVIOUS.is_empty());

const ALIAS: &str = VERSION_CURRENT;
const TIMEOUT: u64 = 10;
const ENABLED: bool = true;
const BYTES: &[u8] = b"wire";
const SIGNED: i16 = -5;

const _: () = assert!(!ALIAS.is_empty());
const _: () = assert!(TIMEOUT > 0);
const _: () = assert!(ENABLED);
const _: () = assert!("fixed".len() == 5);
const _: () = assert!("".is_empty());
const _: () = assert!(!b"wire".is_empty());
const _: () = assert!(BYTES.len() == 4);
const _: () = assert!(b"wire".len() == 4);
const _: () = assert!(true);
const _: () = assert!(-5_i16 < 0);
const _: () = assert!(SIGNED < 0);
const _: () = debug_assert!(!VERSION_CURRENT.is_empty());
static FIXED: () = assert!(10 > 0);

// Independent constants and computed values can encode useful invariants.
const UPPER_BOUND: u64 = 60;
const COMPUTED: usize = 2 + 3;
const FROM_MACRO: &str = concat!("cur", "rent");
const FROM_ENV: &str = env!("CARGO_PKG_NAME");
const _: () = assert!(TIMEOUT < UPPER_BOUND);
const _: () = assert!(VERSION_CURRENT.len() != VERSION_PREVIOUS.len());
const _: () = assert!(ALIAS.len() == VERSION_CURRENT.len());
const _: () = assert!(COMPUTED > 0);
const _: () = assert!(!FROM_MACRO.is_empty());
const _: () = assert!(!FROM_ENV.is_empty());
const _: () = assert!(!generated::VERSION.is_empty());
const _: () = assert!(std::mem::size_of::<u64>() == 8);

struct Invariant<T>(PhantomData<T>);

impl<T> Invariant<T> {
    const SIZE: usize = std::mem::size_of::<T>();
    const CHECK: () = assert!(Self::SIZE > 0);
}

const _: () = assert!(Invariant::<u8>::SIZE > 0);

struct Custom;

impl Custom {
    const fn is_empty(&self) -> bool {
        false
    }
}

const _: () = assert!(!Custom.is_empty());

const fn validate(value: &str) -> &str {
    assert!(!value.is_empty());
    value
}

fn runtime(value: &str) {
    assert!(!value.is_empty());
    assert!(!VERSION_CURRENT.is_empty());
}

// False conditions are not this rule's diagnostic, even in dead const code.
const _: () = {
    if false {
        assert!(false);
        assert!(TIMEOUT == 0);
    }
};

macro_rules! wrapped_assertion {
    () => {
        assert!(!VERSION_CURRENT.is_empty())
    };
}

const _: () = wrapped_assertion!();

fn main() {
    const { assert!(!VERSION_CURRENT.is_empty()) };
}

macro_rules! assert {
    ($condition:expr) => {
        ()
    };
}

const _: () = assert!(true);
