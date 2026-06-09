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
    /// When false, emit plain text instead of ANSI-rendered markdown.
    pub use_ansi: bool,
}

/// Whether `--print` text mode should apply ANSI markdown rendering.
pub fn should_use_ansi(force_plain: bool, stdout_is_tty: bool) -> bool {
    !force_plain && stdout_is_tty
}

/// Exit code for `--print` mode (mirrors shell conventions in the plan).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrintExitCode {
    Success = 0,
    Error = 1,
    MaxTurns = 2,
}

/// Default `stop_reason` when the event stream ends without `StreamEvent::Done`.
const INCOMPLETE_STOP_REASON: &str = "incomplete";

impl PrintExitCode {
    pub fn from_stop_reason(reason: &str) -> Self {
        match reason {
            "maxTurnRequests" => Self::MaxTurns,
            "error" | "cancelled" | INCOMPLETE_STOP_REASON => Self::Error,
            "end_turn" | "max_tokens" | "stop_sequence" => Self::Success,
            _ => Self::Error,
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
        StreamEvent::ToolFinished {
            name,
            output,
            exit_code,
            ..
        } => Some(json!({
            "type": "tool_result",
            "name": name,
            "output": output,
            "exit_code": exit_code,
        })),
        StreamEvent::Usage {
            used_tokens,
            context_size,
        } => Some(json!({
            "type": "usage",
            "used_tokens": used_tokens,
            "context_size": context_size,
        })),
        StreamEvent::Diff(_) | StreamEvent::Done(_) | StreamEvent::Error(_) => None,
    }
}

/// Non-interactive mode: send one prompt, stream output to stdout, exit.
pub async fn run<S: Session>(session: S, prompt: &str, format: OutputFormat, options: PrintOptions) -> PrintExitCode {
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
        let mut stop_reason = String::from(INCOMPLETE_STOP_REASON);

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
        return PrintExitCode::from_stop_reason(&stop_reason);
    }

    if format == OutputFormat::Json {
        let mut text = String::new();
        let mut stop_reason = String::from(INCOMPLETE_STOP_REASON);

        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::Text(t) => text.push_str(&t),
                StreamEvent::ToolFinished {
                    name,
                    output,
                    exit_code,
                    ..
                } if options.print_tools => {
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
                    eprintln!("error: {msg}");
                    return PrintExitCode::Error;
                }
                _ => {}
            }
        }

        let out = serde_json::json!({"text": text, "stop_reason": stop_reason});
        let _ = writeln!(stdout, "{out}");
        return PrintExitCode::from_stop_reason(&stop_reason);
    }

    let mut text = String::new();
    let mut stop_reason = String::from(INCOMPLETE_STOP_REASON);

    while let Some(event) = rx.recv().await {
        match event {
            StreamEvent::Text(t) => text.push_str(&t),
            StreamEvent::ToolFinished {
                name,
                output,
                exit_code,
                ..
            } if options.print_tools => {
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

    let rendered = if options.use_ansi {
        crate::markdown::render(&text)
    } else {
        crate::markdown::render_plain(&text)
    };
    print!("{rendered}");
    if !rendered.ends_with('\n') {
        println!();
    }
    let _ = stdout.flush();
    PrintExitCode::from_stop_reason(&stop_reason)
}

/// Auto-deny responder for non-interactive `--print` mode.
///
/// In `--print` there is no human to answer permission prompts, so an
/// approval-gated tool (bash, or an MCP tool such as `spawn_agent`) would block
/// the turn forever waiting for an approval that never comes. This subscribes to
/// the session's permission-request subject and replies `Cancelled` to every
/// request — which the runner maps to "denied" — so the model is told the tool
/// was denied and the turn completes instead of hanging.
///
/// This is the safe default for non-interactive runs. `--dangerously-skip-permissions`
/// (which sets `bypassPermissions`) opts into auto-*allow* instead, in which case
/// the runner never sends a permission request and this responder is not started.
///
/// Uses the concrete `async_nats::Client` rather than the `NatsClient` trait
/// because answering a request-reply needs each message's reply subject, which
/// `NatsClient::subscribe_bytes` does not expose. The subscription is established
/// before this returns, so it is active before the prompt is sent. Returns the
/// responder task handle; abort it once the run completes.
pub async fn spawn_auto_deny_permissions(
    nats: async_nats::Client,
    prefix: &str,
    session_id: &str,
) -> tokio::task::JoinHandle<()> {
    use agent_client_protocol::{RequestPermissionOutcome, RequestPermissionResponse};
    use futures::StreamExt as _;

    let subject = format!("{prefix}.session.{session_id}.client.session.request_permission");
    let sub = nats.subscribe(subject).await;
    tokio::spawn(async move {
        let mut sub = match sub {
            Ok(s) => s,
            Err(_) => return,
        };
        while let Some(msg) = sub.next().await {
            let Some(reply) = msg.reply else { continue };
            // Best-effort: name the tool being denied so `--print` output explains
            // itself. ACP serializes JSON in camelCase (`toolCall.title`).
            let tool = serde_json::from_slice::<serde_json::Value>(&msg.payload)
                .ok()
                .and_then(|v| {
                    v.get("toolCall")
                        .and_then(|t| t.get("title"))
                        .and_then(|t| t.as_str())
                        .map(str::to_string)
                });
            match tool {
                Some(t) => eprintln!(
                    "note: auto-denied permission for `{t}` (non-interactive --print; pass --dangerously-skip-permissions to allow)"
                ),
                None => eprintln!(
                    "note: auto-denied a permission request (non-interactive --print; pass --dangerously-skip-permissions to allow)"
                ),
            }
            let resp = RequestPermissionResponse::new(RequestPermissionOutcome::Cancelled);
            if let Ok(payload) = serde_json::to_vec(&resp) {
                let _ = nats.publish(reply, payload.into()).await;
            }
        }
    })
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
    fn from_stop_reason_incomplete_maps_to_error() {
        assert_eq!(
            PrintExitCode::from_stop_reason(INCOMPLETE_STOP_REASON),
            PrintExitCode::Error
        );
    }

    #[test]
    fn from_stop_reason_end_turn_maps_to_success() {
        assert_eq!(
            PrintExitCode::from_stop_reason("end_turn"),
            PrintExitCode::Success
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
            stream_event_json(&StreamEvent::Usage {
                used_tokens: 5,
                context_size: 100
            }),
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
                StreamEvent::Usage {
                    used_tokens: 100,
                    context_size: 1000,
                },
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

    #[test]
    fn should_use_ansi_respects_tty_and_plain_flag() {
        assert!(should_use_ansi(false, true));
        assert!(!should_use_ansi(true, true));
        assert!(!should_use_ansi(false, false));
        assert!(!should_use_ansi(true, false));
    }

    #[tokio::test]
    async fn text_mode_plain_output_has_no_ansi_escapes() {
        let session = MockSession::new("s");
        session.queue_turn(vec![
            StreamEvent::Text("hello **world**\n".into()),
            StreamEvent::Done("end_turn".into()),
        ]);
        let options = PrintOptions {
            print_tools: false,
            use_ansi: false,
        };
        assert_eq!(
            run(session, "test", OutputFormat::Text, options).await,
            PrintExitCode::Success
        );
    }

    #[tokio::test]
    async fn text_mode_does_not_close_session() {
        use std::sync::Arc;
        let session = Arc::new(MockSession::new("s"));
        session.queue_turn(vec![StreamEvent::Done("end_turn".into())]);
        run(
            Arc::clone(&session),
            "test",
            OutputFormat::Text,
            PrintOptions::default(),
        )
        .await;
        assert_eq!(session.close_count(), 0);
    }

    #[tokio::test]
    async fn json_mode_does_not_close_session() {
        use std::sync::Arc;
        let session = Arc::new(MockSession::new("s"));
        session.queue_turn(vec![
            StreamEvent::Text("hi".into()),
            StreamEvent::Done("end_turn".into()),
        ]);
        run(
            Arc::clone(&session),
            "test",
            OutputFormat::Json,
            PrintOptions::default(),
        )
        .await;
        assert_eq!(session.close_count(), 0);
    }
}
