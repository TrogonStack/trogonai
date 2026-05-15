use crate::session::{Session, StreamEvent};
use std::io::Write as _;

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
/// Returns `Err` if the prompt cannot be sent or the runner signals stop reason `"error"`.
pub async fn run<S: Session>(session: S, prompt: &str, format: OutputFormat) -> anyhow::Result<()> {
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
                        session.close().await;
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
                        session.close().await;
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

        if !trailing_newline {
            println!();
        }
        let _ = stdout.flush();
    }

    session.close().await;
    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::mock::MockSession;

    #[tokio::test]
    async fn text_mode_returns_ok_on_end_turn() {
        let session = MockSession::new("s");
        session.queue_turn(vec![
            StreamEvent::Text("hello\n".into()),
            StreamEvent::Done("end_turn".into()),
        ]);
        let result = run(session, "test", OutputFormat::Text).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn text_mode_returns_err_on_error_stop_reason() {
        let session = MockSession::new("s");
        session.queue_turn(vec![StreamEvent::Done("error".into())]);
        let result = run(session, "test", OutputFormat::Text).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("error"));
    }

    #[tokio::test]
    async fn json_mode_returns_ok_and_accumulates_text() {
        let session = MockSession::new("s");
        session.queue_turn(vec![
            StreamEvent::Text("foo".into()),
            StreamEvent::Text("bar".into()),
            StreamEvent::Done("end_turn".into()),
        ]);
        // We can't easily capture stdout in tests, but we verify it doesn't error.
        let result = run(session, "test", OutputFormat::Json).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn json_mode_returns_err_on_error_stop_reason() {
        let session = MockSession::new("s");
        session.queue_turn(vec![StreamEvent::Done("error".into())]);
        let result = run(session, "test", OutputFormat::Json).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn non_text_events_are_ignored_in_both_modes() {
        for format in [OutputFormat::Text, OutputFormat::Json] {
            let session = MockSession::new("s");
            session.queue_turn(vec![
                StreamEvent::Thinking,
                StreamEvent::ToolCall("Read".into()),
                StreamEvent::Diff("--- a\n+++ b\n".into()),
                StreamEvent::Usage { used_tokens: 100, context_size: 1000 },
                StreamEvent::Done("end_turn".into()),
            ]);
            let result = run(session, "test", format).await;
            assert!(result.is_ok(), "format={format:?}: {result:?}", format = format as u8);
        }
    }

    #[tokio::test]
    async fn empty_event_stream_completes_without_panic() {
        let session = MockSession::new("s");
        session.queue_turn(vec![StreamEvent::Done("end_turn".into())]);
        let result = run(session, "test", OutputFormat::Text).await;
        assert!(result.is_ok());
    }

    #[tokio::test]
    async fn text_mode_closes_session_on_completion() {
        use std::sync::Arc;
        let session = Arc::new(MockSession::new("s"));
        session.queue_turn(vec![StreamEvent::Done("end_turn".into())]);
        run(Arc::clone(&session), "test", OutputFormat::Text).await.unwrap();
        assert_eq!(session.close_count(), 1, "close() must be called exactly once");
    }

    #[tokio::test]
    async fn text_mode_closes_session_on_error_stop_reason() {
        use std::sync::Arc;
        let session = Arc::new(MockSession::new("s"));
        session.queue_turn(vec![StreamEvent::Done("error".into())]);
        let _ = run(Arc::clone(&session), "test", OutputFormat::Text).await;
        assert_eq!(session.close_count(), 1, "close() must be called even on error stop reason");
    }

    #[tokio::test]
    async fn json_mode_closes_session_on_completion() {
        use std::sync::Arc;
        let session = Arc::new(MockSession::new("s"));
        session.queue_turn(vec![StreamEvent::Text("hi".into()), StreamEvent::Done("end_turn".into())]);
        run(Arc::clone(&session), "test", OutputFormat::Json).await.unwrap();
        assert_eq!(session.close_count(), 1, "close() must be called exactly once");
    }
}
