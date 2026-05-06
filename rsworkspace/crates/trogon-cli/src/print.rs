use crate::session::{StreamEvent, TrogonSession};
use async_nats::Client;
use std::io::Write as _;
use std::path::PathBuf;

/// Non-interactive mode: send one prompt, stream `Text` events to stdout, exit.
///
/// Exit code 0 on success. Returns `Err` (→ exit code 1 in `main`) if the
/// session cannot be created, the prompt cannot be sent, or the runner signals
/// a stop reason of `"error"`.
pub async fn run(nats: Client, prefix: &str, cwd: PathBuf, prompt: &str) -> anyhow::Result<()> {
    let session = TrogonSession::new(nats, prefix, cwd).await?;
    let mut rx = session.prompt(prompt).await?;
    let mut stdout = std::io::stdout();
    let mut trailing_newline = false;

    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::Text(text) => {
                trailing_newline = text.ends_with('\n');
                print!("{text}");
                let _ = stdout.flush();
            }
            StreamEvent::Done(reason) => {
                if reason == "error" {
                    return Err(anyhow::anyhow!("agent stopped with error"));
                }
                break;
            }
            StreamEvent::Thinking
            | StreamEvent::ToolCall(_)
            | StreamEvent::Diff(_)
            | StreamEvent::Usage { .. } => {}
        }
    }

    // Ensure output ends on a clean line.
    if !trailing_newline {
        println!();
    }
    let _ = stdout.flush();
    Ok(())
}

