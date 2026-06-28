// edition:2024
#![allow(unused)]

use std::env;
use std::env::var;

// Fully-qualified read: the violation.
fn fully_qualified() {
    let _ = std::env::var("NATS_URL");
}

// Module-imported read (`use std::env;` then `env::var`): same def, must fire.
fn module_imported() {
    let _ = env::var("NATS_USER");
}

// Function-imported read (`use std::env::var;` then `var`): same def, must fire.
fn function_imported() {
    let _ = var("NATS_PASSWORD");
}

// `ReadEnv` has no equivalent for the OS-string and iterator readers, so
// flagging them would deny code with no alternative: must NOT fire (yet).
fn unsupported_readers() {
    let _ = std::env::var_os("HOME");
    let _ = std::env::vars();
    let _ = std::env::vars_os();
}

// Non-reader `std::env` items are not about variable values: must NOT fire.
fn non_readers() {
    let _ = std::env::current_dir();
    let _ = std::env::args();
}

// A local function that merely shares the name `var` is unrelated: must NOT fire.
fn local_shadow() {
    fn var(_: &str) -> &'static str {
        ""
    }
    let _ = var("NATS_URL");
}

// Explicitly allowed at the site: must NOT fire.
#[allow(std_env_access)]
fn allowed() {
    let _ = std::env::var("NATS_URL");
}

fn main() {}
