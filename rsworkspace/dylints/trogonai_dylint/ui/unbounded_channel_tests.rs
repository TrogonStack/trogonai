// edition:2024
//! UI fixture proving `unbounded_channel` is suppressed in test files. The
//! fixture is named `*_tests.rs`, so the same constructor that fires in
//! `unbounded_channel.rs` produces no diagnostic here. Absent a `.stderr`, the
//! harness asserts zero diagnostics.
#![allow(unused)]

use std::sync::mpsc;

fn pump() {
    let _ = mpsc::channel::<u8>();
}

fn main() {
    pump();
}
