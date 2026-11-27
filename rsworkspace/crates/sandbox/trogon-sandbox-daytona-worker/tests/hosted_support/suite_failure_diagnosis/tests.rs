use trogon_sandbox_protocol::NatsTransferError;

use super::*;

fn mismatch(name: &'static str, expected: &str) -> SuiteFailureError {
    SuiteFailureError {
        name,
        failure: CheckFailureError::Mismatch {
            expected: expected.to_owned(),
            observed: "private guest output".to_owned(),
        },
    }
}

fn transport(name: &'static str) -> SuiteFailureError {
    SuiteFailureError {
        name,
        failure: CheckFailureError::Transport {
            subject: "sandbox.provider.daytona.run_command.x".to_owned(),
            source: NatsTransferError::RequestTimedOut,
        },
    }
}

#[test]
fn no_failures_have_no_markers() {
    assert!(diagnose(&[]).is_empty());
}

#[test]
fn file_mismatches_name_the_step_that_failed() {
    let markers = diagnose(&[
        mismatch(FILES, "the written bytes"),
        mismatch(FILES, "cat to see the written file"),
    ]);
    assert_eq!(
        markers,
        vec!["daytona_suite_files_bytes_mismatch", "daytona_suite_files_cat_mismatch"]
    );
}

#[test]
fn kill_and_egress_mismatches_are_classified() {
    let markers = diagnose(&[
        mismatch(KILL, "a killed command not to exit zero"),
        mismatch(EGRESS, "CONNECTED or DENIED"),
    ]);
    assert_eq!(
        markers,
        vec![
            "daytona_suite_kill_exited_zero",
            "daytona_suite_egress_probe_inconclusive"
        ]
    );
}

#[test]
fn transport_failures_are_classified_by_check() {
    assert_eq!(diagnose(&[transport(KILL)]), vec!["daytona_suite_kill_transport"]);
}

#[test]
fn an_unknown_failure_is_diagnostic_only_and_never_echoes_output() {
    let markers = diagnose(&[mismatch("lease_is_granted", "a grant")]);
    assert_eq!(markers, vec![DIAGNOSTIC_ONLY]);
    assert!(markers.iter().all(|marker| !marker.contains("private")));
}

#[test]
fn repeats_collapse_and_the_failure_count_is_bounded() {
    let failures: Vec<_> = (0..MAX_FAILURES + 10)
        .map(|_| mismatch(FILES, "the written bytes"))
        .collect();
    assert_eq!(diagnose(&failures), vec!["daytona_suite_files_bytes_mismatch"]);
}

#[test]
fn a_fragment_past_the_reason_bound_is_not_matched() {
    let mut expected = "x".repeat(MAX_REASON_BYTES);
    expected.push_str("the written bytes");
    assert_eq!(diagnose(&[mismatch(FILES, &expected)]), vec![DIAGNOSTIC_ONLY]);
}
