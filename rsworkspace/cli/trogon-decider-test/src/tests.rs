use super::*;

fn args(format: OutputFormat, no_strict: bool, wasm: PathBuf, suite: PathBuf) -> Args {
    Args {
        format,
        no_strict,
        wasm,
        suite,
    }
}

fn schedules_wasm_path() -> PathBuf {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/wasm32-unknown-unknown/release/trogon_schedules_decider.wasm");
    assert!(
        path.exists(),
        "build trogon_schedules_decider.wasm for wasm32-unknown-unknown first (expected {})",
        path.display()
    );
    path
}

fn schedules_suite_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("schedules.yaml")
}

#[test]
fn run_passes_the_checked_in_schedules_suite() {
    run(args(
        OutputFormat::Human,
        false,
        schedules_wasm_path(),
        schedules_suite_path(),
    ))
    .expect("schedules suite passes");
}

#[test]
fn run_fails_when_the_wasm_path_does_not_exist() {
    let error = run(args(
        OutputFormat::Human,
        false,
        PathBuf::from("/nonexistent/trogon_schedules_decider.wasm"),
        schedules_suite_path(),
    ))
    .unwrap_err()
    .to_string();
    assert!(error.contains("read"), "unexpected error: {error}");
}
