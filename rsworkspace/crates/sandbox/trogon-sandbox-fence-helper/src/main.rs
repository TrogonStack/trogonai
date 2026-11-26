//! `trogon-sandbox-fence-helper`: the standalone binary, a thin shim over the
//! library's `run_from_args` entry point. The dispatch lives in `cli.rs` so an
//! embedding multicall binary can link the crate and route to it when
//! `argv[1] == DISPATCH_ARG1` without shipping a separate executable. See
//! `cli.rs` for why dispatch keys on `argv[1]`, not `argv[0]`, on Windows.

#[cfg(windows)]
fn main() {
    std::process::exit(trogon_sandbox_fence_helper::run_from_args(std::env::args_os()));
}

#[cfg(not(windows))]
fn main() {
    eprintln!("trogon-sandbox-fence-helper: Windows only");
    std::process::exit(2);
}
