use super::*;

fn output(stdout: usize, stderr: usize) -> CommandOutput {
    CommandOutput {
        exit_code: 0,
        stdout: vec![b'a'; stdout],
        stderr: vec![b'b'; stderr],
        ..CommandOutput::default()
    }
}

fn completed(reply: RunCommandResponse) -> Option<CommandOutput> {
    match reply {
        RunCommandResponse::Completed(output) => Some(output),
        RunCommandResponse::Denied(_) | RunCommandResponse::Started(_) => None,
    }
}

#[test]
fn fitting_output_is_returned_unchanged() {
    let original = output(4, 4);
    let reply = fit_reply(original.clone(), 1 << 20);
    assert!(matches!(reply, RunCommandResponse::Completed(fitted) if fitted == original));
}

#[test]
fn oversized_output_is_trimmed_and_marked() -> Result<(), Box<dyn std::error::Error>> {
    let fitted = completed(fit_reply(output(40_000, 40_000), 20_000)).ok_or("expected a trimmed reply")?;
    assert!(fitted.truncated.is_truncated());
    assert!(encoded_len(&fitted) <= 20_000);
    assert!(fitted.stdout.len() < 40_000 && fitted.stderr.len() < 40_000);
    Ok(())
}

#[test]
fn a_tight_payload_still_gets_a_marked_reply() {
    let empty = encoded_len(&output(0, 0));
    let reply = fit_reply(output(5_000, 0), empty + MARKER_ROOM + 64);
    assert!(matches!(reply, RunCommandResponse::Completed(_)));
}

#[test]
fn a_payload_whose_bytes_cost_more_than_average_shrinks_until_it_fits() -> Result<(), Box<dyn std::error::Error>> {
    let mut stdout = vec![0u8; 36_000];
    stdout.extend(vec![255u8; 4_000]);
    let original = CommandOutput { stdout, ..output(0, 0) };
    let fitted = completed(fit_reply(original, 20_000)).ok_or("expected a trimmed reply")?;
    assert!(encoded_len(&fitted) <= 20_000);
    Ok(())
}

#[test]
fn an_envelope_larger_than_the_payload_is_refused() {
    let reply = fit_reply(output(100, 0), 10);
    assert!(completed(reply.clone()).is_none());
    assert!(matches!(
        reply,
        RunCommandResponse::Denied(LeaseError::CommandOutputTooLargeForTransport {
            stdout_bytes: 100,
            max_payload_bytes: 10,
            ..
        })
    ));
}

#[test]
fn output_without_streams_is_refused_when_it_cannot_fit() {
    let reply = fit_reply(output(0, 0), 20);
    assert!(matches!(reply, RunCommandResponse::Denied(_)));
}

#[test]
fn the_budget_goes_to_the_stream_that_needs_it() {
    assert_eq!(split_budget(10, 2, 100), (2, 8));
    assert_eq!(split_budget(10, 100, 2), (8, 2));
    assert_eq!(split_budget(10, 100, 100), (5, 5));
}
