//! The re-executions the Linux isolation depends on: the Landlock confiner,
//! the filesystem observer and the egress relay run as this program's own
//! executable inside the sandbox. A program that embeds the sandbox and points the backend at
//! itself calls [`run_if_invoked`] first thing in `main`, before any thread
//! or async runtime exists.

use clap::Parser;

use super::{guest_init, landlock, nested, observer, relay};

/// Runs the helper this process was started as, if it was started as one,
/// and returns the exit code to leave with. `None` means an ordinary start.
pub fn run_if_invoked() -> Option<i32> {
    let argv: Vec<_> = std::env::args_os().collect();
    if guest_init::is_init_invocation(&argv) {
        return Some(match guest_init::run(guest_init::InitArgs::parse()) {
            Ok(code) => code,
            Err(err) => {
                super::diagnostics::to_stderr(format_args!("init: {err:#}"));
                guest_init::INIT_EXIT_CODE
            }
        });
    }
    if nested::is_nested_invocation(&argv) {
        return Some(match nested::run(nested::NestedArgs::parse()) {
            Ok(code) => code,
            Err(err) => {
                super::diagnostics::to_stderr(format_args!("nested: {err:#}"));
                nested::NESTED_EXIT_CODE
            }
        });
    }
    if landlock::is_confine_invocation(&argv) {
        return Some(match landlock::run(landlock::ConfineArgs::parse()) {
            Ok(never) => match never {},
            Err(err) => {
                super::diagnostics::to_stderr(format_args!("confine: {err:#}"));
                landlock::CONFINE_EXIT_CODE
            }
        });
    }
    if observer::is_observe_invocation(&argv) {
        return Some(match observer::run(observer::ObserveArgs::parse()) {
            Ok(code) => code,
            Err(err) => {
                super::diagnostics::to_stderr(format_args!("observe: {err:#}"));
                1
            }
        });
    }
    if relay::is_relay_invocation(&argv) {
        return Some(match relay::run(relay::RelayArgs::parse()) {
            Ok(code) => code,
            Err(err) => {
                super::diagnostics::to_stderr(format_args!("relay: {err:#}"));
                relay::RELAY_EXIT_CODE
            }
        });
    }
    None
}
