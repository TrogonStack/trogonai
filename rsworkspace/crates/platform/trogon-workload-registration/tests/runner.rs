#![allow(clippy::expect_used)]

#[cfg(unix)]
#[path = "runner/input_boundary_tests.rs"]
mod input_boundary_tests;

#[test]
fn administrative_startup_requires_explicit_scope_fence_and_approval_key() {
    for args in [
        vec!["serve"],
        vec!["withdraw"],
        vec!["provision"],
        vec!["serve", "--recovery-fence", "0"],
    ] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_trogon-workload-registration"))
            .args(args)
            .env_clear()
            .stdin(std::process::Stdio::null())
            .output()
            .expect("execute public administrative binary");
        assert!(
            !output.status.success(),
            "an unconfigured administrative entry point must refuse startup"
        );
    }
}
