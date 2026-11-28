mod common;

use common::TestResult;
use trogon_sandbox_contract::{CommandOutput, LeaseError, PollCommandResponse, ReadFileResponse, RunCommandResponse};
use trogon_sandbox_protocol::{
    COMMAND_REPLY_MARKER_ROOM, encode_read_file_response_within, fit_poll_command_response, fit_run_command_response,
};

fn output(stdout: usize, stderr: usize) -> CommandOutput {
    CommandOutput {
        exit_code: 0,
        stdout: vec![b'a'; stdout],
        stderr: vec![b'b'; stderr],
        ..CommandOutput::default()
    }
}

fn encoded_len(output: &CommandOutput) -> usize {
    serde_json::to_vec(&RunCommandResponse::Completed(output.clone())).map_or(usize::MAX, |body| body.len())
}

fn run(output: CommandOutput, max_payload: usize) -> RunCommandResponse {
    fit_run_command_response(RunCommandResponse::Completed(output), max_payload)
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
    let reply = run(original.clone(), 1 << 20);
    assert!(matches!(reply, RunCommandResponse::Completed(fitted) if fitted == original));
}

#[test]
fn oversized_output_is_trimmed_and_marked() -> TestResult {
    let fitted = completed(run(output(40_000, 40_000), 20_000)).ok_or("expected a trimmed reply")?;
    assert!(fitted.truncated.is_truncated());
    assert!(encoded_len(&fitted) <= 20_000);
    assert!(fitted.stdout.len() < 40_000 && fitted.stderr.len() < 40_000);
    Ok(())
}

#[test]
fn a_tight_payload_still_gets_a_marked_reply() {
    let empty = encoded_len(&output(0, 0));
    let reply = run(output(5_000, 0), empty + COMMAND_REPLY_MARKER_ROOM + 64);
    assert!(matches!(reply, RunCommandResponse::Completed(_)));
}

#[test]
fn a_payload_whose_bytes_cost_more_than_average_shrinks_until_it_fits() -> TestResult {
    let mut stdout = vec![0u8; 36_000];
    stdout.extend(vec![255u8; 4_000]);
    let original = CommandOutput { stdout, ..output(0, 0) };
    let fitted = completed(run(original, 20_000)).ok_or("expected a trimmed reply")?;
    assert!(encoded_len(&fitted) <= 20_000);
    Ok(())
}

#[test]
fn an_envelope_larger_than_the_payload_is_refused() {
    let reply = run(output(100, 0), 10);
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
    let reply = run(output(0, 0), 20);
    assert!(matches!(reply, RunCommandResponse::Denied(_)));
}

#[test]
fn a_small_stream_is_kept_whole_while_the_large_one_is_trimmed() -> TestResult {
    let fitted = completed(run(output(10, 40_000), 20_000)).ok_or("expected a trimmed reply")?;
    assert_eq!(fitted.stdout, vec![b'a'; 10]);
    assert!(fitted.stderr.len() < 40_000);
    let fitted = completed(run(output(40_000, 10), 20_000)).ok_or("expected a trimmed reply")?;
    assert_eq!(fitted.stderr, vec![b'b'; 10]);
    assert!(fitted.stdout.len() < 40_000);
    Ok(())
}

#[test]
fn a_reply_that_is_not_finished_output_passes_through() {
    assert_eq!(
        fit_run_command_response(
            RunCommandResponse::Denied(LeaseError::InvalidSpec { reason: "x".into() }),
            1
        ),
        RunCommandResponse::Denied(LeaseError::InvalidSpec { reason: "x".into() })
    );
    for reply in [PollCommandResponse::Running, PollCommandResponse::NotFound] {
        assert_eq!(fit_poll_command_response(reply.clone(), 1), reply);
    }
}

#[test]
fn a_poll_reply_is_trimmed_like_a_run_reply() -> TestResult {
    let PollCommandResponse::Completed(fitted) =
        fit_poll_command_response(PollCommandResponse::Completed(output(40_000, 0)), 20_000)
    else {
        return Err("expected a trimmed poll reply".into());
    };
    assert!(fitted.truncated.is_truncated());
    assert!(serde_json::to_vec(&PollCommandResponse::Completed(fitted))?.len() <= 20_000);
    assert!(matches!(
        fit_poll_command_response(PollCommandResponse::Completed(output(100, 0)), 10),
        PollCommandResponse::Denied(LeaseError::CommandOutputTooLargeForTransport { .. })
    ));
    Ok(())
}

#[test]
fn a_file_that_fits_is_returned_and_one_that_does_not_is_refused() -> TestResult {
    let content = ReadFileResponse::Content { data: vec![1; 100] };
    assert_eq!(
        encode_read_file_response_within(&content, "/a", 1 << 20)?,
        serde_json::to_vec(&content)?
    );
    let refused: ReadFileResponse = serde_json::from_slice(&encode_read_file_response_within(&content, "/a", 50)?)?;
    assert!(matches!(
        refused,
        ReadFileResponse::Denied(LeaseError::FileTooLargeForTransport {
            bytes: 100,
            max_payload_bytes: 50,
            ..
        })
    ));
    assert_eq!(
        encode_read_file_response_within(&ReadFileResponse::NotFound, "/a", 1)?,
        serde_json::to_vec(&ReadFileResponse::NotFound)?
    );
    Ok(())
}
