use crate::session::{StreamEvent, TrogonSession};
use async_nats::Client;
use std::path::PathBuf;
use tokio::sync::mpsc;

/// Non-interactive mode: send one prompt, print TextDelta to stdout, exit.
/// Exit code 0 on success, 1 on error.
pub async fn run(nats: Client, prefix: &str, cwd: PathBuf, prompt: &str) -> anyhow::Result<()> {
    let session: TrogonSession = TrogonSession::new(nats, prefix, cwd).await?;
    let mut rx: mpsc::Receiver<StreamEvent> = session.prompt(prompt).await?;

    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::Text(text) => print!("{text}"),
            StreamEvent::Done(_) => break,
            StreamEvent::Thinking | StreamEvent::ToolCall(_) => {}
        }
    }

    Ok(())
}
