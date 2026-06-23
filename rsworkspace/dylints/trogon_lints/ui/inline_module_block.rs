// edition:2024

// The lint is `Allow` by default (disabled pending migration); enable it here
// so the UI test still exercises the detection and `#[allow]` handling.
#![deny(inline_module_block)]

// Inline module with a body block: this is the violation.
mod twin {
    pub fn run() {}
}

// Another inline block, nested inside a function-free module tree.
mod tests {
    #[test]
    fn it_works() {}
}

// Nested inline modules: BOTH the outer and the inner block must be flagged.
mod outer {
    mod inner {
        pub fn run() {}
    }
}

// Explicitly allowed at the site: must NOT be linted.
#[allow(inline_module_block)]
mod allowed_inline {
    pub fn run() {}
}

// `#[allow]` on a parent must suppress nested inline modules too: neither
// `allowed_outer` nor `allowed_inner` may be linted.
#[allow(inline_module_block)]
mod allowed_outer {
    mod allowed_inner {
        pub fn run() {}
    }
}

// Macro-generated inline modules come from expansion and must NOT be linted.
macro_rules! make_mod {
    ($name:ident) => {
        mod $name {
            pub fn run() {}
        }
    };
}

make_mod!(generated);

// File-backed declaration (`mod foo;`): the body lives in a separate file, so
// the lint must NOT fire. This is the central negative the policy promises.
#[path = "auxiliary/file_backed.rs"]
mod file_backed;

fn main() {}
