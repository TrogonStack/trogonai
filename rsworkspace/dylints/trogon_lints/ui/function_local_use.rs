// edition:2024
#![allow(unused)]

// Module-level imports are fine: must NOT be linted.
use std::fmt::Write as _;

// Simple function-local `use`: this is the violation.
fn render(value: u8) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = write!(out, "{value}");
    out
}

// Glob function-local `use`: also flagged.
fn step(input: std::cmp::Ordering) -> i32 {
    use std::cmp::Ordering::*;
    match input {
        Less => -1,
        Equal => 0,
        Greater => 1,
    }
}

// List function-local `use`: lowers to several HIR items sharing one statement
// span, so it must be reported exactly ONCE.
fn listed() {
    use std::fmt::{Debug, Display};
}

// `use` inside a nested block is still function-local: flagged.
fn nested() {
    {
        use std::fmt::Debug;
    }
}

// Explicitly allowed at the site: must NOT be linted.
#[allow(function_local_use)]
fn allowed() {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = write!(out, "ok");
}

// Macro-generated imports come from expansion and must NOT be linted.
macro_rules! make_use {
    () => {
        fn generated() {
            use std::fmt::Write;
            let mut out = String::new();
            let _ = write!(out, "gen");
        }
    };
}

make_use!();

fn main() {}
