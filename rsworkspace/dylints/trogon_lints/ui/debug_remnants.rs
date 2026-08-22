// edition:2024
#![allow(unused)]

use std::fmt::Write as _;

// Each printing macro must fire once, at its own call site.
fn stdout_writes() {
    println!("state: {:?}", 1);
    print!("progress");
}

fn stderr_writes() {
    eprintln!("failed: {}", "boom");
}

fn inspection() {
    let _ = dbg!(1 + 1);
}

// A single invocation expands to several nodes: the lint must report it once.
fn repeated_expansion_nodes() {
    println!("{} {} {}", 1, 2, 3);
}

// `eprint!` draws a prompt or a progress line, which a log event cannot
// replace: must NOT fire.
fn prompt() {
    eprint!("password: ");
}

// `write!`/`writeln!` name their sink, so nothing goes to the process's stdio:
// must NOT fire.
fn explicit_sink() {
    let mut buffer = String::new();
    let _ = writeln!(buffer, "value");
}

// A local macro that merely shares a name with `std`'s is unrelated: must NOT
// fire. `macro_rules!` is textually scoped, so this shadows `println!` only for
// the code below it.
macro_rules! println {
    ($($arg:tt)*) => {
        ()
    };
}

fn local_shadow() {
    println!("not std's");
}

// Explicitly allowed at the site: must NOT fire.
#[allow(debug_remnants)]
fn allowed() {
    std::println!("deliberate output");
}

fn main() {}
