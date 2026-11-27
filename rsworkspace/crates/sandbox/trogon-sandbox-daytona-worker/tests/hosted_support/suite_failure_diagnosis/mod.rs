//! Turns the typed failures of a conformance run into a short list of fixed markers, so a hosted report names what
//! failed without carrying guest output.

use trogon_sandbox_conformance::{CheckFailureError, SuiteFailureError};

/// Most failures the diagnosis looks at.
pub const MAX_FAILURES: usize = 64;

/// Most bytes of an expectation the diagnosis matches against.
pub const MAX_REASON_BYTES: usize = 8 * 1024;

/// Reported for a failure no marker describes.
pub const DIAGNOSTIC_ONLY: &str = "daytona_suite_failure_diagnostic_only";

const FILES: &str = "files_round_trip_and_commands_see_them";
const KILL: &str = "kill_stops_a_running_command";
const EGRESS: &str = "denied_connect_fails";

/// Expectation fragments per check, with the marker each one maps to.
const RULES: &[(&str, &[(&str, &str)])] = &[
    (
        FILES,
        &[
            ("the file to be written", "daytona_suite_files_write_refused"),
            ("the written bytes", "daytona_suite_files_bytes_mismatch"),
            ("cat to see", "daytona_suite_files_cat_mismatch"),
        ],
    ),
    (
        KILL,
        &[
            ("the command running", "daytona_suite_kill_not_running"),
            ("signaled", "daytona_suite_kill_not_signaled"),
            ("not to exit zero", "daytona_suite_kill_exited_zero"),
            ("nothing to kill", "daytona_suite_kill_already_finished"),
        ],
    ),
    (
        EGRESS,
        &[
            ("concrete egress probe endpoint", "daytona_suite_egress_no_endpoint"),
            ("unrestricted", "daytona_suite_egress_control_blocked"),
            ("not to open a connection", "daytona_suite_egress_denied_connected"),
            ("CONNECTED or DENIED", "daytona_suite_egress_probe_inconclusive"),
            ("nc or a Python3", "daytona_suite_egress_no_probe_tool"),
        ],
    ),
];

fn bounded(text: &str) -> &str {
    let mut end = text.len().min(MAX_REASON_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn transport_marker(name: &str) -> Option<&'static str> {
    match name {
        FILES => Some("daytona_suite_files_transport"),
        KILL => Some("daytona_suite_kill_transport"),
        EGRESS => Some("daytona_suite_egress_transport"),
        _ => None,
    }
}

fn marker(failure: &SuiteFailureError) -> Option<&'static str> {
    match &failure.failure {
        CheckFailureError::Transport { .. } => transport_marker(failure.name),
        CheckFailureError::Mismatch { expected, .. } => {
            let expected = bounded(expected);
            RULES
                .iter()
                .filter(|(name, _)| *name == failure.name)
                .flat_map(|(_, rules)| rules.iter())
                .find(|(fragment, _)| expected.contains(fragment))
                .map(|(_, marker)| *marker)
        }
        _ => None,
    }
}

/// The markers for `failures`, each once, in first-seen order. A failure no marker describes adds
/// `DIAGNOSTIC_ONLY`.
pub fn diagnose(failures: &[SuiteFailureError]) -> Vec<&'static str> {
    let mut markers: Vec<&'static str> = Vec::new();
    for failure in failures.iter().take(MAX_FAILURES) {
        let found = marker(failure).unwrap_or(DIAGNOSTIC_ONLY);
        if !markers.contains(&found) {
            markers.push(found);
        }
    }
    markers
}

#[cfg(test)]
mod tests;
