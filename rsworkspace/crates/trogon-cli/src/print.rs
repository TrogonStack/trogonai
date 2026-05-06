use crate::session::{StreamEvent, TrogonSession};
use async_nats::Client;
use std::io::Write as _;
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Text,
    Json,
}

/// Non-interactive mode: send one prompt, stream output to stdout, exit.
///
/// `Text` mode streams `Text` events as plain text, flushing per chunk.
/// `Json` mode accumulates all text then emits a single JSON line:
///   `{"text":"...","stop_reason":"end_turn"}`
///
/// Returns `Err` (→ exit code 1 in `main`) if the session cannot be created,
/// the prompt cannot be sent, or the runner signals stop reason `"error"`.
pub async fn run(
    nats: Client,
    prefix: &str,
    cwd: PathBuf,
    prompt: &str,
    format: OutputFormat,
) -> anyhow::Result<()> {
    let session = TrogonSession::new(nats, prefix, cwd).await?;
    let mut rx = session.prompt(prompt).await?;
    let mut stdout = std::io::stdout();

    if format == OutputFormat::Json {
        let mut text = String::new();
        let mut stop_reason = String::from("end_turn");

        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::Text(t) => text.push_str(&t),
                StreamEvent::Done(reason) => {
                    if reason == "error" {
                        return Err(anyhow::anyhow!("agent stopped with error"));
                    }
                    stop_reason = reason;
                    break;
                }
                StreamEvent::Thinking
                | StreamEvent::ToolCall(_)
                | StreamEvent::Diff(_)
                | StreamEvent::Usage { .. } => {}
            }
        }

        let out = serde_json::json!({"text": text, "stop_reason": stop_reason});
        writeln!(stdout, "{out}")?;
    } else {
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
    }

    Ok(())
}
