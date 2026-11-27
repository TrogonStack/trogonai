use trogon_sandbox_contract::{
    CommandOutput, FileOpErrorKind, FileOpResponse, LeaseError, PollCommandResponse, ReadFileResponse,
    RunCommandResponse,
};

use super::{encode_file_op_within, encode_poll_within, encode_read_within, encode_run_within};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn output(stdout: usize, stderr: usize) -> CommandOutput {
    CommandOutput {
        exit_code: 4,
        stdout: vec![b'o'; stdout],
        stderr: vec![b'e'; stderr],
        ..Default::default()
    }
}

fn run_completed(encoded: &[u8]) -> Result<CommandOutput, Box<dyn std::error::Error>> {
    match serde_json::from_slice(encoded)? {
        RunCommandResponse::Completed(output) => Ok(output),
        other => Err(format!("expected a completed run, got {other:?}").into()),
    }
}

#[test]
fn a_run_reply_that_fits_is_sent_unchanged() -> TestResult {
    let response = RunCommandResponse::Completed(output(10, 10));

    let encoded = encode_run_within(response.clone(), 1 << 20)?;

    assert_eq!(serde_json::from_slice::<RunCommandResponse>(&encoded)?, response);
    Ok(())
}

#[test]
fn a_run_reply_that_is_not_a_completion_is_never_trimmed() -> TestResult {
    let response = RunCommandResponse::Denied(LeaseError::Backend {
        message: "x".repeat(500),
    });

    let encoded = encode_run_within(response.clone(), 10)?;

    assert_eq!(serde_json::from_slice::<RunCommandResponse>(&encoded)?, response);
    Ok(())
}

#[test]
fn oversized_run_output_is_trimmed_and_flagged() -> TestResult {
    let encoded = encode_run_within(RunCommandResponse::Completed(output(4000, 4000)), 2000)?;

    assert!(encoded.len() <= 2000);
    let trimmed = run_completed(&encoded)?;
    assert!(trimmed.truncated.is_truncated());
    assert_eq!(trimmed.exit_code, 4);
    assert!(!trimmed.stdout.is_empty() && !trimmed.stderr.is_empty());
    Ok(())
}

#[test]
fn a_small_stream_keeps_all_of_its_output_when_the_other_is_trimmed() -> TestResult {
    let small_stdout = encode_run_within(RunCommandResponse::Completed(output(10, 6000)), 1500)?;
    let small_stderr = encode_run_within(RunCommandResponse::Completed(output(6000, 10)), 1500)?;

    assert_eq!(run_completed(&small_stdout)?.stdout.len(), 10);
    assert_eq!(run_completed(&small_stderr)?.stderr.len(), 10);
    Ok(())
}

#[test]
fn a_run_reply_that_cannot_fit_even_empty_becomes_a_denial_keeping_the_exit_code() -> TestResult {
    let encoded = encode_run_within(RunCommandResponse::Completed(output(100, 100)), 20)?;

    assert_eq!(
        serde_json::from_slice::<RunCommandResponse>(&encoded)?,
        RunCommandResponse::Denied(LeaseError::CommandOutputTooLargeForTransport {
            exit_code: 4,
            stdout_bytes: 100,
            stderr_bytes: 100,
            max_payload_bytes: 20,
        })
    );
    Ok(())
}

#[test]
fn a_completion_with_nothing_to_trim_becomes_a_denial() -> TestResult {
    let encoded = encode_run_within(RunCommandResponse::Completed(output(0, 0)), 20)?;

    assert!(matches!(
        serde_json::from_slice::<RunCommandResponse>(&encoded)?,
        RunCommandResponse::Denied(LeaseError::CommandOutputTooLargeForTransport { .. })
    ));
    Ok(())
}

#[test]
fn a_poll_reply_is_sent_whole_trimmed_or_denied() -> TestResult {
    let fits = encode_poll_within(PollCommandResponse::Completed(output(5, 5)), 1 << 20)?;
    let trimmed = encode_poll_within(PollCommandResponse::Completed(output(4000, 4000)), 2000)?;
    let denied = encode_poll_within(PollCommandResponse::Completed(output(100, 100)), 20)?;
    let running = encode_poll_within(PollCommandResponse::Running, 1)?;

    assert_eq!(
        serde_json::from_slice::<PollCommandResponse>(&fits)?,
        PollCommandResponse::Completed(output(5, 5))
    );
    assert!(matches!(
        serde_json::from_slice::<PollCommandResponse>(&trimmed)?,
        PollCommandResponse::Completed(ref output) if output.truncated.is_truncated()
    ));
    assert!(matches!(
        serde_json::from_slice::<PollCommandResponse>(&denied)?,
        PollCommandResponse::Denied(LeaseError::CommandOutputTooLargeForTransport { .. })
    ));
    assert_eq!(
        serde_json::from_slice::<PollCommandResponse>(&running)?,
        PollCommandResponse::Running
    );
    Ok(())
}

#[test]
fn a_file_that_fits_is_sent_and_one_that_does_not_is_denied_with_its_size() -> TestResult {
    let content = ReadFileResponse::Content { data: vec![1; 500] };

    let fits = encode_read_within(content.clone(), "/workspace/a", 1 << 20)?;
    let too_large = encode_read_within(content, "/workspace/a", 100)?;

    assert!(matches!(
        serde_json::from_slice::<ReadFileResponse>(&fits)?,
        ReadFileResponse::Content { ref data } if data.len() == 500
    ));
    assert_eq!(
        serde_json::from_slice::<ReadFileResponse>(&too_large)?,
        ReadFileResponse::Denied(LeaseError::FileTooLargeForTransport {
            path: "/workspace/a".to_string(),
            bytes: 500,
            max_payload_bytes: 100,
        })
    );
    Ok(())
}

#[test]
fn a_file_reply_that_is_not_content_is_never_replaced() -> TestResult {
    let encoded = encode_read_within(ReadFileResponse::NotFound, "/workspace/a", 1)?;

    assert_eq!(
        serde_json::from_slice::<ReadFileResponse>(&encoded)?,
        ReadFileResponse::NotFound
    );
    Ok(())
}

#[test]
fn a_file_op_reply_that_fits_is_sent_unchanged() -> TestResult {
    let response = FileOpResponse::NotFound {
        path: "/workspace/a".to_owned(),
    };

    let encoded = encode_file_op_within(response.clone(), 1 << 20)?;

    assert_eq!(serde_json::from_slice::<FileOpResponse>(&encoded)?, response);
    Ok(())
}

#[test]
fn an_oversized_file_op_reply_becomes_a_typed_transport_size_failure() -> TestResult {
    let response = FileOpResponse::NotFound { path: "p".repeat(500) };

    let encoded = encode_file_op_within(response, 200)?;

    assert!(encoded.len() <= 200);
    assert!(matches!(
        serde_json::from_slice::<FileOpResponse>(&encoded)?,
        FileOpResponse::Failed {
            kind: FileOpErrorKind::TooLarge,
            ..
        }
    ));
    Ok(())
}
