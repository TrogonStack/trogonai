#[cfg(target_os = "linux")]
mod guest_init_error;
#[cfg(target_os = "linux")]
mod linux;

use std::io::Write as _;

#[cfg(target_os = "linux")]
fn main() -> std::process::ExitCode {
    let Err(error) = linux::run();
    let _ = writeln!(std::io::stderr(), "guest init: {error}");
    linux::linger_as_init(std::process::id(), linux::park_once);
    std::process::ExitCode::FAILURE
}

#[cfg(not(target_os = "linux"))]
fn main() {
    let _ = writeln!(std::io::stderr(), "trogon-sandbox-guest-init requires Linux");
    std::process::exit(1);
}
