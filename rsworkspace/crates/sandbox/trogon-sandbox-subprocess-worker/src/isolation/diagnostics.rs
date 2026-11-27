//! Where the re-executed helpers say why they stopped. The helpers run inside
//! the sandbox with no logging pipeline; their standard error is what reaches
//! the host.

use std::fmt::Arguments;
use std::io::Write;

/// Writes one line to standard error. A failed write has nowhere else to go.
pub(super) fn to_stderr(message: Arguments<'_>) {
    let _ = writeln!(std::io::stderr().lock(), "{message}");
}
