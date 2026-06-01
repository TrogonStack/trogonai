use crate::session::{Session, StreamEvent};
use std::io::Write as _;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Text,
    /// Single JSON object at the end: `{"text":..., "stop_reason":...}`.
    Json,
    /// NDJSON: one JSON event per line as the turn streams, ending with a
    /// `{"type":"result",...}` line.
    StreamJson,
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

/// Map a stream event to its NDJSON line for `--output-format stream-json`.
/// Returns `None` for events that are not emitted on their own (`Diff` is a TUI
/// rendering; `Done`/`Error` drive control flow and are handled by the caller).
pub(crate) fn stream_event_json(event: &StreamEvent) -> Option<serde_json::Value> {
    use serde_json::json;
    match event {
        StreamEvent::Text(t) => Some(json!({ "type": "text", "text": t })),
        StreamEvent::Thinking => Some(json!({ "type": "thinking" })),
        StreamEvent::ToolCall(name) => Some(json!({ "type": "tool_use", "name": name })),
        StreamEvent::ToolFinished { name, output, exit_code, .. } => Some(json!({
            "type": "tool_result",
            "name": name,
            "output": output,
            "exit_code": exit_code,
        })),
        StreamEvent::Usage { used_tokens, context_size } => Some(json!({
            "type": "usage",
            "used_tokens": used_tokens,
            "context_size": context_size,
        })),
        StreamEvent::Diff(_) | StreamEvent::Done(_) | StreamEvent::Error(_) => None,
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

    if format == OutputFormat::StreamJson {
        // NDJSON event stream: emit one JSON object per event as it arrives, then
        // a final `result` line. Unlike text/json this is always complete (tool
        // events are not gated on `--print-tools`).
        let mut text = String::new();
        let mut stop_reason = String::from("end_turn");

        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::Done(reason) => {
                    stop_reason = reason;
                    break;
                }
                StreamEvent::Error(msg) => {
                    let _ = writeln!(stdout, "{}", serde_json::json!({ "type": "error", "message": msg }));
                    let _ = writeln!(
                        stdout,
                        "{}",
                        serde_json::json!({ "type": "result", "stop_reason": "error", "text": text })
                    );
                    let _ = stdout.flush();
                    session.close().await;
                    return PrintExitCode::Error;
                }
                other => {
                    if let StreamEvent::Text(t) = &other {
                        text.push_str(t);
                    }
                    if let Some(line) = stream_event_json(&other) {
                        let _ = writeln!(stdout, "{line}");
                    }
                }
            }
        }

        let _ = writeln!(
            stdout,
            "{}",
            serde_json::json!({ "type": "result", "stop_reason": stop_reason, "text": text })
        );
        let _ = stdout.flush();
        session.close().await;
        return PrintExitCode::from_stop_reason(&stop_reason);
    }

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

    #[test]
    fn stream_event_json_maps_each_event_type() {
        use serde_json::json;
        assert_eq!(
            stream_event_json(&StreamEvent::Text("hi".into())),
            Some(json!({"type":"text","text":"hi"}))
        );
        assert_eq!(
            stream_event_json(&StreamEvent::Thinking),
            Some(json!({"type":"thinking"}))
        );
        assert_eq!(
            stream_event_json(&StreamEvent::ToolCall("Read".into())),
            Some(json!({"type":"tool_use","name":"Read"}))
        );
        assert_eq!(
            stream_event_json(&StreamEvent::ToolFinished {
                name: "Bash".into(),
                output: "ok".into(),
                exit_code: Some(0),
                status: agent_client_protocol::ToolCallStatus::Completed,
            }),
            Some(json!({"type":"tool_result","name":"Bash","output":"ok","exit_code":0}))
        );
        assert_eq!(
            stream_event_json(&StreamEvent::Usage { used_tokens: 5, context_size: 100 }),
            Some(json!({"type":"usage","used_tokens":5,"context_size":100}))
        );
        // Diff/Done/Error produce no standalone line.
        assert!(stream_event_json(&StreamEvent::Diff("d".into())).is_none());
        assert!(stream_event_json(&StreamEvent::Done("end_turn".into())).is_none());
        assert!(stream_event_json(&StreamEvent::Error("boom".into())).is_none());
    }

    #[tokio::test]
    async fn stream_json_mode_returns_success_on_end_turn() {
        let session = MockSession::new("s");
        session.queue_turn(vec![
            StreamEvent::Text("foo".into()),
            StreamEvent::ToolCall("Read".into()),
            StreamEvent::Done("end_turn".into()),
        ]);
        assert_eq!(
            run(session, "test", OutputFormat::StreamJson, PrintOptions::default()).await,
            PrintExitCode::Success
        );
    }

    #[tokio::test]
    async fn stream_json_mode_returns_error_on_error_event() {
        let session = MockSession::new("s");
        session.queue_turn(vec![StreamEvent::Error("api failed".into())]);
        assert_eq!(
            run(session, "test", OutputFormat::StreamJson, PrintOptions::default()).await,
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
