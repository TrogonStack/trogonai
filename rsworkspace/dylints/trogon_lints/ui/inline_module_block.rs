// edition:2024

// Inline module with a body block: this is the violation.
mod twin {
    pub fn run() {}
}

// Another inline block, nested inside a function-free module tree.
mod tests {
    #[test]
    fn it_works() {}
}

// Explicitly allowed at the site: must NOT be linted.
#[allow(inline_module_block)]
mod allowed_inline {
    pub fn run() {}
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

// A file-backed declaration (`mod foo;`) is the correct form and must NOT be
// linted; it is exercised by the workspace, not this single-file fixture.

fn main() {}
