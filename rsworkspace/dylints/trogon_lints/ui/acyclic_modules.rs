// edition:2024

// A single-file fixture has to spell its module tree inline, which
// `inline_module_block` forbids in real code and which is irrelevant here.
#![allow(dead_code, inline_module_block, unused_imports)]

// Mutual dependency between two crate-root siblings, closing across
// grandchildren: `payments::checkout` reaches into `server::auth`, and
// `server::auth` reaches back into `payments::billing`. Neither pair of leaves
// is itself a cycle; the cycle is `payments` <-> `server`.
mod payments {
    pub mod checkout {
        pub fn process() {
            let _ = crate::server::auth::verify();
        }
    }

    pub mod billing {
        pub struct Invoice {
            pub amount: u64,
        }

        pub fn create_invoice() -> Invoice {
            Invoice { amount: 0 }
        }
    }
}

mod server {
    pub mod auth {
        pub fn verify() -> bool {
            let _ = crate::payments::billing::create_invoice();
            true
        }
    }

    pub mod routes {
        pub fn index() -> &'static str {
            "ok"
        }
    }
}

// A cycle below the crate root: `shop::checkout` and `shop::billing` are
// siblings under `shop`, so the cycle is reported against `shop`.
mod shop {
    pub mod checkout {
        pub struct CartItem {
            pub name: &'static str,
        }

        pub fn finalize() {
            let _ = crate::shop::billing::total();
        }
    }

    pub mod billing {
        pub fn total() -> u64 {
            let _item = crate::shop::checkout::CartItem { name: "" };
            0
        }
    }
}

// A cycle of three: caught the same way, and reported with the help text for
// more than a pair.
mod pipeline {
    pub mod ingest {
        pub fn run() {
            let _ = crate::pipeline::transform::run();
        }
    }

    pub mod transform {
        pub fn run() {
            let _ = crate::pipeline::load::run();
        }
    }

    pub mod load {
        pub fn run() {
            crate::pipeline::ingest::run();
        }
    }
}

// A `use` import and a type annotation are dependencies too, not just call
// expressions.
mod imports {
    pub mod reader {
        use crate::imports::writer::Record;

        pub struct Reader;

        pub fn read() -> Record {
            Record
        }
    }

    pub mod writer {
        pub struct Record;

        pub fn write(reader: crate::imports::reader::Reader) -> u64 {
            let _ = reader;
            0
        }
    }
}

// One-directional dependency: `consumer` depends on `utils` twice and `utils`
// depends on nothing, so there is no cycle to report.
mod utils {
    pub fn helper() -> u64 {
        42
    }
}

mod consumer {
    pub fn first() -> u64 {
        crate::utils::helper()
    }

    pub fn second() -> u64 {
        crate::utils::helper()
    }
}

// Parent and child reference each other in both directions, which is how the
// module tree is built rather than a cycle in it: nothing is reported.
mod parent {
    pub use child::Config;

    pub mod child {
        pub struct Config;

        pub fn parent_limit() -> u64 {
            super::limit()
        }
    }

    pub fn limit() -> u64 {
        10
    }

    pub fn build() -> Config {
        Config
    }
}

// The cycle between `deliberate::left` and `deliberate::right` is opted out of
// on the module that owns both, so nothing is reported for it.
#[expect(acyclic_modules, reason = "fixture for the documented opt-out")]
mod deliberate {
    pub mod left {
        pub fn go() {
            crate::deliberate::right::go();
        }
    }

    pub mod right {
        pub fn go() {
            crate::deliberate::left::go();
        }
    }
}

// Test code reaches across the tree by design, so a cycle that only exists
// between test modules is not reported.
mod first_tests {
    pub fn cross() {
        crate::second_tests::cross();
    }
}

mod second_tests {
    pub fn cross() {
        crate::first_tests::cross();
    }
}

// A macro that generates a cross-module path writes it on the macro author's
// behalf, not this call site's, so the expansion is not a dependency.
macro_rules! reach_across {
    () => {
        crate::generated_target::target()
    };
}

mod generated_source {
    pub fn run() -> u64 {
        reach_across!()
    }
}

mod generated_target {
    pub fn target() -> u64 {
        crate::generated_source::run()
    }
}

fn main() {}
