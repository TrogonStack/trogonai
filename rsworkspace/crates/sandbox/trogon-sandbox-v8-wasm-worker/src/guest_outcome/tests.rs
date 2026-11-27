use trogon_sandbox_contract::OutputStream;

use super::*;

fn output(text: &str) -> CapturedOutput {
    let mut output = CapturedOutput::default();
    output.lines.push((OutputStream::Stdout, text.to_owned()));
    output
}

#[test]
fn every_outcome_hands_back_its_captured_output() {
    let outcomes = [
        GuestOutcome::Completed { output: output("done") },
        GuestOutcome::failed(output("done"), "boom"),
        GuestOutcome::timed_out(output("done")),
        GuestOutcome::cancelled(output("done")),
    ];

    for outcome in outcomes {
        assert_eq!(outcome.into_output(), output("done"));
    }
}
