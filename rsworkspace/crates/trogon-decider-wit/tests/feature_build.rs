//! Feature-build guards for `trogon-decider-wit`.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

/// A `cargo` command for nested feature/target build checks.
///
/// Strips the coverage instrumentation the parent process carries under `cargo
/// llvm-cov`. llvm-cov injects `-C instrument-coverage --cfg=coverage` through a
/// `RUSTC_WRAPPER` (cargo-llvm-cov itself) plus its `__CARGO_LLVM_COV_*` env, not
/// via `RUSTFLAGS`. Left in place, that both pulls in `profiler_builtins` (absent
/// for `wasm32-unknown-unknown`) and reshapes the build, so the nested check no
/// longer reflects the real feature/target behavior under test.
fn cargo_check() -> std::process::Command {
    let mut cmd = std::process::Command::new("cargo");
    cmd.env_remove("RUSTC_WRAPPER")
        .env_remove("RUSTC_WORKSPACE_WRAPPER")
        .env_remove("__CARGO_LLVM_COV_RUSTC_WRAPPER")
        .env_remove("__CARGO_LLVM_COV_RUSTC_WRAPPER_RUSTFLAGS")
        .env_remove("__CARGO_LLVM_COV_RUSTC_WRAPPER_CRATE_NAMES")
        .env_remove("LLVM_PROFILE_FILE")
        .env_remove("RUSTFLAGS")
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."));
    cmd
}

#[test]
fn guest_feature_builds_on_wasm32() {
    let status = cargo_check()
        .args([
            "check",
            "-p",
            "trogon-decider-wit",
            "--features",
            "guest",
            "--target",
            "wasm32-unknown-unknown",
        ])
        .status()
        .expect("cargo check");
    assert!(status.success(), "guest feature must build for wasm32-unknown-unknown");
}

#[test]
fn host_feature_builds_on_native() {
    let status = cargo_check()
        .args(["check", "-p", "trogon-decider-wit", "--features", "host"])
        .status()
        .expect("cargo check");
    assert!(status.success(), "host feature must build on native");
}

#[test]
fn host_feature_fails_on_wasm32() {
    let output = cargo_check()
        .args([
            "check",
            "-p",
            "trogon-decider-wit",
            "--features",
            "host",
            "--target",
            "wasm32-unknown-unknown",
        ])
        .output()
        .expect("cargo check");
    assert!(!output.status.success(), "host feature must not build for wasm32");
}
