use std::collections::HashMap;

use trogon_sandbox_protocol::{DispatchVerifier, DispatchVerifierConfigError, ENV_FENCE_FILE, ENV_VERIFY_MODE};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn lookup(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

#[test]
fn a_configured_fence_journal_that_cannot_be_opened_is_a_startup_error() -> TestResult {
    let directory = tempfile::tempdir()?;
    let blocker = directory.path().join("blocker");
    std::fs::write(&blocker, "file")?;
    let journal = blocker.join("fence.jsonl").to_string_lossy().into_owned();
    let env = lookup(&[(ENV_VERIFY_MODE, "warn"), (ENV_FENCE_FILE, &journal)]);
    let result = DispatchVerifier::from_lookup(&|name| env.get(name).cloned(), None);
    assert!(
        matches!(result, Err(DispatchVerifierConfigError::FenceFile { .. })),
        "{:?}",
        result.err()
    );
    Ok(())
}

#[test]
fn an_unusable_default_fence_journal_still_leaves_the_verifier_in_memory() -> TestResult {
    let directory = tempfile::tempdir()?;
    let blocker = directory.path().join("blocker");
    std::fs::write(&blocker, "file")?;
    let default = blocker.join("fence.jsonl");
    DispatchVerifier::from_lookup(&|_| None, Some(default))?;
    Ok(())
}
