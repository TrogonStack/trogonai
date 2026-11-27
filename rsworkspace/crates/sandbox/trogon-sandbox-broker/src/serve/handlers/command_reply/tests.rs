use trogon_sandbox_contract::{CommandOutput, LeaseError};

use super::*;

fn output(stdout: usize) -> CommandOutput {
    CommandOutput {
        stdout: vec![b'a'; stdout],
        ..CommandOutput::default()
    }
}

#[test]
fn a_poll_reply_that_fits_is_unchanged() {
    let reply = PollCommandResponse::Completed(output(16));
    assert_eq!(fit_poll_command_response(reply.clone(), 1024 * 1024), reply);
}

#[test]
fn an_oversized_poll_reply_is_trimmed() -> Result<(), Box<dyn std::error::Error>> {
    let max_payload = 4096;
    let fitted = fit_poll_command_response(PollCommandResponse::Completed(output(64 * 1024)), max_payload);
    let PollCommandResponse::Completed(trimmed) = &fitted else {
        return Err(format!("expected a trimmed reply, got {fitted:?}").into());
    };
    assert!(trimmed.truncated.is_truncated());
    assert!(serde_json::to_vec(&fitted)?.len() <= max_payload);
    Ok(())
}

#[test]
fn a_poll_reply_that_cannot_fit_is_denied() {
    let fitted = fit_poll_command_response(PollCommandResponse::Completed(output(64 * 1024)), 8);
    assert!(matches!(fitted, PollCommandResponse::Denied(_)));
}

#[test]
fn replies_without_output_pass_through() {
    for reply in [
        PollCommandResponse::Running,
        PollCommandResponse::NotFound,
        PollCommandResponse::Denied(LeaseError::Backend {
            message: "unavailable".to_owned(),
        }),
    ] {
        assert_eq!(fit_poll_command_response(reply.clone(), 8), reply);
    }
}
