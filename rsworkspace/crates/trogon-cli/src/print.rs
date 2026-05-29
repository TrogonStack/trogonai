use crate::session::{Session, StreamEvent};
use std::io::Write as _;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Text,
    Json,
}

#[derive(Clone, Copy, Default)]
pub struct PrintOptions {
    pub print_tools: bool,
}

/// Exit code for `--print` mode (mirrors shell conventions in the plan).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrintExitCode {
    Success = 0,
    Error = 1,
    MaxTurns = 2,
}

impl PrintExitCode {
    pub fn from_stop_reason(reason: &str) -> Self {
        match reason {
            "maxTurnRequests" => Self::MaxTurns,
            "error" | "cancelled" => Self::Error,
            _ => Self::Success,
        }
    }
}

/// Non-interactive mode: send one prompt, stream output to stdout, exit.
pub async fn run<S: Session>(
    session: S,
    prompt: &str,
    format: OutputFormat,
    options: PrintOptions,
) -> PrintExitCode {
    let mut rx = match session.prompt(prompt).await {
        Ok(rx) => rx,
        Err(e) => {
            eprintln!("error: {e}");
            return PrintExitCode::Error;
        }
    };
    let mut stdout = std::io::stdout();

    if format == OutputFormat::Json {
        let mut text = String::new();
        let mut stop_reason = String::from("end_turn");

        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::Text(t) => text.push_str(&t),
                StreamEvent::ToolFinished { name, output, exit_code, .. } if options.print_tools => {
                    let line = serde_json::json!({
                        "type": "tool",
                        "name": name,
                        "output": output,
                        "exit_code": exit_code,
                    });
                    let _ = writeln!(stdout, "{line}");
                }
                StreamEvent::Done(reason) => {
                    stop_reason = reason;
                    break;
                }
                StreamEvent::Error(msg) => {
                    session.close().await;
                    eprintln!("error: {msg}");
                    return PrintExitCode::Error;
                }
                _ => {}
            }
        }

        let out = serde_json::json!({"text": text, "stop_reason": stop_reason});
        let _ = writeln!(stdout, "{out}");
        session.close().await;
        return PrintExitCode::from_stop_reason(&stop_reason);
    }

    let mut text = String::new();
    let mut stop_reason = String::from("end_turn");

    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::Text(t) => text.push_str(&t),
            StreamEvent::ToolFinished { name, output, exit_code, .. } if options.print_tools => {
                let line = serde_json::json!({
                    "type": "tool",
                    "name": name,
                    "output": output,
                    "exit_code": exit_code,
                });
                let _ = writeln!(stdout, "{line}");
            }
            StreamEvent::Done(reason) => {
                stop_reason = reason;
                break;
            }
            StreamEvent::Error(msg) => {
                session.close().await;
                eprintln!("error: {msg}");
                return PrintExitCode::Error;
            }
            StreamEvent::Thinking
            | StreamEvent::ToolCall(_)
            | StreamEvent::Diff(_)
            | StreamEvent::ToolFinished { .. }
            | StreamEvent::Usage { .. } => {}
        }
    }

    let rendered = crate::markdown::render(&text);
    print!("{rendered}");
    if !rendered.ends_with('\n') {
        println!();
    }
    let _ = stdout.flush();
    session.close().await;
    PrintExitCode::from_stop_reason(&stop_reason)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::mock::MockSession;

    #[tokio::test]
    async fn text_mode_returns_success_on_end_turn() {
        let session = MockSession::new("s");
        session.queue_turn(vec![
            StreamEvent::Text("hello\n".into()),
            StreamEvent::Done("end_turn".into()),
        ]);
        assert_eq!(
            run(session, "test", OutputFormat::Text, PrintOptions::default()).await,
            PrintExitCode::Success
        );
    }

    #[tokio::test]
    async fn text_mode_returns_error_on_error_stop_reason() {
        let session = MockSession::new("s");
        session.queue_turn(vec![StreamEvent::Done("error".into())]);
        assert_eq!(
            run(session, "test", OutputFormat::Text, PrintOptions::default()).await,
            PrintExitCode::Error
        );
    }

    #[tokio::test]
    async fn max_turn_requests_yields_exit_code_2() {
        let session = MockSession::new("s");
        session.queue_turn(vec![StreamEvent::Done("maxTurnRequests".into())]);
        assert_eq!(
            run(session, "test", OutputFormat::Text, PrintOptions::default()).await,
            PrintExitCode::MaxTurns
        );
    }

    #[tokio::test]
    async fn json_mode_returns_success_and_accumulates_text() {
        let session = MockSession::new("s");
        session.queue_turn(vec![
            StreamEvent::Text("foo".into()),
            StreamEvent::Text("bar".into()),
            StreamEvent::Done("end_turn".into()),
        ]);
        assert_eq!(
            run(session, "test", OutputFormat::Json, PrintOptions::default()).await,
            PrintExitCode::Success
        );
    }

    #[tokio::test]
    async fn json_mode_returns_error_on_error_stop_reason() {
        let session = MockSession::new("s");
        session.queue_turn(vec![StreamEvent::Done("error".into())]);
        assert_eq!(
            run(session, "test", OutputFormat::Json, PrintOptions::default()).await,
            PrintExitCode::Error
        );
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
            assert_eq!(
                run(session, "test", format, PrintOptions::default()).await,
                PrintExitCode::Success
            );
        }
    }

    #[tokio::test]
    async fn empty_event_stream_completes_without_panic() {
        let session = MockSession::new("s");
        session.queue_turn(vec![StreamEvent::Done("end_turn".into())]);
        assert_eq!(
            run(session, "test", OutputFormat::Text, PrintOptions::default()).await,
            PrintExitCode::Success
        );
    }

    #[tokio::test]
    async fn text_mode_closes_session_on_completion() {
        use std::sync::Arc;
        let session = Arc::new(MockSession::new("s"));
        session.queue_turn(vec![StreamEvent::Done("end_turn".into())]);
        run(Arc::clone(&session), "test", OutputFormat::Text, PrintOptions::default()).await;
        assert_eq!(session.close_count(), 1);
    }

    #[tokio::test]
    async fn text_mode_closes_session_on_error_stop_reason() {
        use std::sync::Arc;
        let session = Arc::new(MockSession::new("s"));
        session.queue_turn(vec![StreamEvent::Done("error".into())]);
        run(Arc::clone(&session), "test", OutputFormat::Text, PrintOptions::default()).await;
        assert_eq!(session.close_count(), 1);
    }

    #[tokio::test]
    async fn json_mode_closes_session_on_completion() {
        use std::sync::Arc;
        let session = Arc::new(MockSession::new("s"));
        session.queue_turn(vec![StreamEvent::Text("hi".into()), StreamEvent::Done("end_turn".into())]);
        run(Arc::clone(&session), "test", OutputFormat::Json, PrintOptions::default()).await;
        assert_eq!(session.close_count(), 1);
    }
}
