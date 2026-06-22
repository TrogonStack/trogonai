//! Feature-build guards for `trogon-decider-wit`.
#![allow(clippy::expect_used, clippy::panic, clippy::unwrap_used)]

#[test]
fn guest_feature_builds_on_wasm32() {
    let status = std::process::Command::new("cargo")
        .args([
            "check",
            "-p",
            "trogon-decider-wit",
            "--features",
            "guest",
            "--target",
            "wasm32-unknown-unknown",
        ])
        .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
        .status()
        .expect("cargo check");
    assert!(status.success(), "guest feature must build for wasm32-unknown-unknown");
}

#[test]
fn host_feature_builds_on_native() {
    let status = std::process::Command::new("cargo")
        .args(["check", "-p", "trogon-decider-wit", "--features", "host"])
        .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
        .status()
        .expect("cargo check");
    assert!(status.success(), "host feature must build on native");
}

#[test]
fn host_feature_fails_on_wasm32() {
    let output = std::process::Command::new("cargo")
        .args([
            "check",
            "-p",
            "trogon-decider-wit",
            "--features",
            "host",
            "--target",
            "wasm32-unknown-unknown",
        ])
        .current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
        .output()
        .expect("cargo check");
    assert!(!output.status.success(), "host feature must not build for wasm32");
}
