#![allow(clippy::expect_used)]

#[path = "runner/subprocess_environment.rs"]
mod subprocess_environment;

#[cfg(unix)]
#[path = "runner/input_boundary_tests.rs"]
mod input_boundary_tests;

#[cfg(unix)]
#[path = "runner/broker_tests.rs"]
mod broker_tests;

#[test]
fn administrative_startup_requires_explicit_scope_fence_and_approval_key() {
    for args in [
        vec!["serve"],
        vec!["withdraw"],
        vec!["provision"],
        vec!["serve", "--recovery-fence", "0"],
    ] {
        let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_trogon-workload-registration"));
        command.args(args).env_clear().stdin(std::process::Stdio::null());
        subprocess_environment::preserve_coverage(&mut command);
        let output = command.output().expect("execute public administrative binary");
        assert!(
            !output.status.success(),
            "an unconfigured administrative entry point must refuse startup"
        );
    }
}
