// edition:2024
#![allow(unused)]

use std::sync::mpsc;

// `std::sync::mpsc::channel` is the unbounded half of the pair: fires.
fn fully_qualified() {
    let _ = std::sync::mpsc::channel::<u8>();
}

// The same def reached through a module import: fires.
fn module_imported() {
    let _ = mpsc::channel::<u8>();
}

// `sync_channel` is the one that takes a capacity: must NOT fire.
fn bounded() {
    let _ = mpsc::sync_channel::<u8>(16);
}

// A local function that merely shares the name `channel` is unrelated: must
// NOT fire.
fn local_shadow() {
    fn channel() {}
    channel();
}

// Explicitly allowed at the site: must NOT fire.
#[allow(unbounded_channel)]
fn allowed() {
    let _ = mpsc::channel::<u8>();
}

// `main` is not exempt: a queue wired up during composition grows the same way
// as one wired up anywhere else. Fires.
// A queue built by the test-support module family is scaffolding: must NOT
// fire.
#[allow(inline_module_block)]
mod mocks {
    use std::sync::mpsc;

    pub fn pump() {
        let _ = mpsc::channel::<u8>();
    }
}

fn main() {
    let _ = mpsc::channel::<u8>();
}
