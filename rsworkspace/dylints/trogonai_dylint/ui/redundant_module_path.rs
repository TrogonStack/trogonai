// edition:2024

// Redundant: a crate root owns its own directory, so `mod tests;` already
// resolves to the sibling `tests.rs`. The `#[path]` is noise and must fire.
#[path = "tests.rs"]
mod tests;

// Load-bearing: the body lives outside the default location, so the attribute
// is doing real work and must NOT fire.
#[path = "auxiliary/file_backed.rs"]
mod relocated;

// `#[allow]` at the site must suppress the lint.
#[allow(redundant_module_path)]
#[path = "allowed.rs"]
mod allowed;

// Non-crate-root owner: `owner.rs` is itself a file-backed module, so its child
// `mod inner;` resolves into the `owner/` subdirectory. The `#[path]` inside
// `owner.rs` spelling that default must fire, exercising the non-root branch.
#[path = "auxiliary/owner.rs"]
mod owner;

fn main() {}
