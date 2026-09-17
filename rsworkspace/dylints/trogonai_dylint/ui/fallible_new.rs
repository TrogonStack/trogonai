// edition:2024
#![allow(unused)]

struct Endpoint {
    port: u16,
}

struct Listener {
    port: u16,
}

struct Registry {
    slots: Vec<u16>,
}

// `.unwrap()` on a `Result` inside `new`: fires.
impl Endpoint {
    fn new(raw: &str) -> Self {
        Self {
            port: raw.parse::<u16>().unwrap(),
        }
    }
}

// `.expect(...)` on an `Option` inside `new`: fires.
impl Listener {
    fn new(port: Option<u16>) -> Self {
        Self {
            port: port.expect("port must be given"),
        }
    }
}

// A bare `panic!` inside `new`: fires.
impl Registry {
    fn new(capacity: usize) -> Self {
        if capacity == 0 {
            panic!("capacity must not be zero");
        }
        Self {
            slots: Vec::with_capacity(capacity),
        }
    }

    // An `unreachable!` is a panic the caller still cannot handle: fires.
    fn new_from_pair(pair: (u16, u16)) -> Self {
        match pair {
            (0, _) => unreachable!("the first slot is never zero"),
            (first, second) => Self {
                slots: vec![first, second],
            },
        }
    }

    // A `new_*` variant carries the same promise as `new`: fires.
    fn new_with_capacity(capacity: Option<usize>) -> Self {
        Self {
            slots: Vec::with_capacity(capacity.unwrap()),
        }
    }

    // The panic sits in a closure the constructor writes, so it is still the
    // constructor's own code: fires.
    fn new_mapped(raw: &str) -> Self {
        Self {
            slots: raw.split(',').map(|slot| slot.parse::<u16>().unwrap()).collect(),
        }
    }

    // Returning `Result` puts the failure in the signature: must NOT fire.
    fn new_checked(raw: &str) -> Result<Self, String> {
        let slot = raw.parse::<u16>().map_err(|error| error.to_string())?;
        Ok(Self { slots: vec![slot] })
    }

    // `Option` says construction can fail just as `Result` does: must NOT fire.
    fn new_optional(raw: &str) -> Option<Self> {
        Some(Self {
            slots: vec![raw.parse::<u16>().unwrap()],
        })
    }

    // `try_new` already tells the caller construction is fallible, and it
    // returns a `Result`: must NOT fire.
    fn try_new(raw: &str) -> Result<Self, String> {
        Ok(Self {
            slots: vec![raw.parse::<u16>().unwrap()],
        })
    }

    // Nothing in the body can panic: must NOT fire.
    fn new_empty() -> Self {
        Self { slots: Vec::new() }
    }

    // `todo!` is left to rustc's own lint for unfinished code: must NOT fire.
    fn new_unwritten() -> Self {
        todo!("decide the slot layout")
    }

    // An `unwrap` on a type that is neither `Result` nor `Option` discards
    // nothing: must NOT fire.
    fn new_unwrapped(slot: Slot) -> Self {
        Self {
            slots: vec![slot.unwrap()],
        }
    }

    // Explicitly allowed at the site: must NOT fire.
    #[allow(fallible_new)]
    fn new_allowed(raw: &str) -> Self {
        Self {
            slots: vec![raw.parse::<u16>().unwrap()],
        }
    }
}

struct Slot(u16);

impl Slot {
    fn unwrap(self) -> u16 {
        self.0
    }
}

trait Build {
    fn new(raw: &str) -> Self;
}

// The implementor owns neither the name nor the return type: must NOT fire.
impl Build for Slot {
    fn new(raw: &str) -> Self {
        Self(raw.parse::<u16>().unwrap())
    }
}

// A function that is not a constructor at all: must NOT fire.
fn parse(raw: &str) -> u16 {
    raw.parse::<u16>().unwrap()
}

struct Connection {
    port: u16,
}

impl Connection {
    // An `async` constructor promises the same thing the blocking one does:
    // fires.
    async fn new(raw: &str) -> Self {
        Self {
            port: raw.parse::<u16>().unwrap(),
        }
    }

    // An `async` constructor returning `Result` admits failure: must NOT fire.
    async fn new_checked(raw: &str) -> Result<Self, String> {
        let port = raw.parse::<u16>().map_err(|error| error.to_string())?;
        Ok(Self { port })
    }
}

// The test-support module family is scaffolding, so a constructor declared
// directly in one is exempt whether it sits in an `impl` or at module scope:
// must NOT fire.
#[allow(inline_module_block)]
mod mocks {
    pub struct StubPort {
        port: u16,
    }

    pub fn new(raw: &str) -> StubPort {
        StubPort {
            port: raw.parse::<u16>().unwrap(),
        }
    }

    impl StubPort {
        pub fn new_stub(raw: &str) -> Self {
            Self {
                port: raw.parse::<u16>().unwrap(),
            }
        }
    }
}

fn main() {}
