use std::io::Write as _;
use std::process::ExitCode;

use trogon_sandbox_capability_matrix::{Mode, check_doc, write_doc};

fn main() -> ExitCode {
    let result = Mode::from_args(std::env::args().skip(1)).and_then(|mode| match mode {
        Mode::Write => write_doc(),
        Mode::Check => check_doc(),
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            let _ = writeln!(std::io::stderr(), "{err}");
            ExitCode::FAILURE
        }
    }
}
