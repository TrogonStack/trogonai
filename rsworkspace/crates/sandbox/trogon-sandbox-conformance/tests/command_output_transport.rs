//! A command whose retained output exceeds one bus message still answers with a truncated success over a real NATS
//! server.

use std::error::Error;
use std::time::Duration;

use futures::StreamExt;
use trogon_nats::test_support::CoreTestServer;
use trogon_sandbox_contract::{
    CommandOutput, CommandTimeoutStatus, OutputTruncation, PollCommandResponse, RunCommandResponse,
};
use trogon_sandbox_protocol::nats_transfer;
use trogon_sandbox_protocol::{DEFAULT_MAX_OUTPUT_BYTES, cap_buffered};

type Sendable<T> = Result<T, Box<dyn Error + Send + Sync>>;

const WAIT: Duration = Duration::from_secs(10);

/// Runs a real command that prints three times the default retention cap and keeps its output the way a worker does.
async fn oversized_command_output() -> Result<CommandOutput, Box<dyn Error>> {
    let ran = tokio::process::Command::new("sh")
        .args(["-c", "head -c 3000000 /dev/zero | tr '\\0' a; echo done >&2; exit 3"])
        .output()
        .await?;
    let (stdout, out_cut) = cap_buffered(ran.stdout, DEFAULT_MAX_OUTPUT_BYTES);
    let (stderr, err_cut) = cap_buffered(ran.stderr, DEFAULT_MAX_OUTPUT_BYTES);
    Ok(CommandOutput {
        exit_code: ran.status.code().ok_or("the command was killed by a signal")?,
        stdout,
        stderr,
        timed_out: CommandTimeoutStatus::WithinDeadline,
        truncated: OutputTruncation::from(out_cut.is_truncated() || err_cut.is_truncated()),
    })
}

fn assert_truncated_success(output: &CommandOutput) -> Result<(), Box<dyn Error>> {
    assert_eq!(output.exit_code, 3);
    assert!(output.truncated.is_truncated());
    assert_eq!(output.stderr, b"done\n");
    let text = std::str::from_utf8(&output.stdout)?;
    assert!(text.starts_with('a') && text.ends_with('a'), "head and tail kept");
    assert_eq!(text.matches("bytes truncated").count(), 1);
    Ok(())
}

async fn answer(client: &async_nats::Client, subscriber: &mut async_nats::Subscriber, body: Vec<u8>) -> Sendable<()> {
    let message = tokio::time::timeout(WAIT, subscriber.next())
        .await?
        .ok_or("the subscription closed")?;
    let reply = message.reply.ok_or("the request has no reply subject")?;
    nats_transfer::respond(client, reply.as_str(), body.into()).await?;
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn oversized_command_output_arrives_truncated_instead_of_refused() -> Result<(), Box<dyn Error>> {
    let server = CoreTestServer::start().await;
    let client = async_nats::connect(server.address()).await?;
    let output = oversized_command_output().await?;
    let run_body = serde_json::to_vec(&RunCommandResponse::Completed(output.clone()))?;
    let poll_body = serde_json::to_vec(&PollCommandResponse::Completed(output))?;
    assert!(
        run_body.len() > client.server_info().max_payload,
        "the reply needs more than one message"
    );

    let mut run = client.subscribe("test.run_command").await?;
    let mut poll = client.subscribe("test.poll_command").await?;
    client.flush().await?;
    let responder = {
        let client = client.clone();
        tokio::spawn(async move {
            answer(&client, &mut run, run_body).await?;
            answer(&client, &mut poll, poll_body).await
        })
    };

    let ran = nats_transfer::request(&client, "test.run_command", Default::default(), None, WAIT).await?;
    match serde_json::from_slice::<RunCommandResponse>(&ran)? {
        RunCommandResponse::Completed(output) => assert_truncated_success(&output)?,
        other => return Err(format!("expected a truncated success, got {other:?}").into()),
    }
    let polled = nats_transfer::request(&client, "test.poll_command", Default::default(), None, WAIT).await?;
    match serde_json::from_slice::<PollCommandResponse>(&polled)? {
        PollCommandResponse::Completed(output) => assert_truncated_success(&output)?,
        other => return Err(format!("expected a truncated success, got {other:?}").into()),
    }
    responder.await?.map_err(|error| error.to_string())?;
    Ok(())
}
