//! Maps `StreamEvent` → terminal output for one assistant turn.
//!
//! This reproduces the streaming render behavior of the REPL: streamed vs
//! buffered (markdown-rendered) text, the live `┆` tool line, tool output,
//! diffs, and the end-of-turn token footer. When a new `StreamEvent` variant
//! is added, add a match arm here.

use std::io::Write as _;

use agent_client_protocol::ToolCallStatus;

use crate::repl::fmt_tokens;
use crate::session::StreamEvent;
use crate::terminal::{print_tool_output, reset_display};

/// Token counters updated during a turn.
#[derive(Debug, Clone, Copy, Default)]
pub struct TurnMetrics {
    pub used_tokens: u64,
    pub context_size: u64,
}

/// A tool finished with output the REPL may use to sync its cwd.
#[derive(Debug, Clone)]
pub struct CwdSync {
    pub tool_name: String,
    pub output: String,
}

/// Why the turn loop should stop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnStop {
    Done { reason: String },
    Error(String),
}

/// Renders streaming events for one assistant turn.
pub struct TurnRenderer {
    /// When true, text tokens print as they arrive; otherwise they are buffered
    /// and markdown-rendered when the turn (or a tool call) interrupts the text.
    stream: bool,
    response_buf: String,
    /// True while a `┆` tool line is live on stderr (no trailing newline).
    tool_line_active: bool,
    /// True after the first text token — `reset_display` runs once per text run.
    text_started: bool,
    stop: Option<TurnStop>,
}

impl TurnRenderer {
    pub fn new(stream: bool) -> Self {
        Self {
            stream,
            response_buf: String::new(),
            tool_line_active: false,
            text_started: false,
            stop: None,
        }
    }

    pub fn take_stop(&mut self) -> Option<TurnStop> {
        self.stop.take()
    }

    pub fn is_stopped(&self) -> bool {
        self.stop.is_some()
    }

    pub fn on_ctrl_c(&mut self) {
        self.clear_tool_line();
        eprintln!("\n[cancelled]");
    }

    pub fn handle(&mut self, event: StreamEvent, metrics: &mut TurnMetrics) -> Option<CwdSync> {
        let mut stdout = std::io::stdout();
        let mut cwd_sync = None;
        match event {
            StreamEvent::Text(text) => {
                if self.tool_line_active {
                    // clear tool line before text starts
                    eprint!("\r\x1b[2K\n");
                    let _ = std::io::stderr().flush();
                    self.tool_line_active = false;
                    self.text_started = false;
                }
                if !self.text_started {
                    reset_display();
                    self.text_started = true;
                }
                if self.stream {
                    print!("{text}");
                    let _ = stdout.flush();
                } else {
                    self.response_buf.push_str(&text);
                }
            }
            StreamEvent::Thinking => {}
            StreamEvent::ToolCall(name) => {
                self.flush_buffered_markdown();
                if self.tool_line_active {
                    // overwrite the previous tool line in place
                    eprint!("\r\x1b[2K\x1b[2m┆ {name}\x1b[0m");
                } else {
                    eprint!("\n\x1b[2m┆ {name}\x1b[0m");
                    self.tool_line_active = true;
                }
                let _ = std::io::stderr().flush();
            }
            StreamEvent::Diff(diff) => {
                reset_display();
                eprintln!("{diff}");
            }
            StreamEvent::ToolFinished {
                name,
                output,
                exit_code,
                status,
            } => {
                self.clear_tool_line();
                let failed = matches!(status, ToolCallStatus::Failed);
                let code_suffix = exit_code.map(|c| format!(" (exit {c})")).unwrap_or_default();
                let badge = if failed { " [failed]" } else { "" };
                eprintln!("\x1b[2m┆ {name}{code_suffix}{badge}\x1b[0m");
                if !output.is_empty() {
                    print_tool_output(&output);
                    cwd_sync = Some(CwdSync {
                        tool_name: name,
                        output,
                    });
                }
            }
            StreamEvent::Usage {
                used_tokens,
                context_size,
            } => {
                metrics.used_tokens = used_tokens;
                metrics.context_size = context_size;
            }
            StreamEvent::Error(msg) => {
                self.clear_tool_line();
                self.flush_buffered_markdown();
                eprintln!("\n\x1b[31merror: {msg}\x1b[0m");
                let _ = stdout.flush();
                self.stop = Some(TurnStop::Error(msg));
            }
            StreamEvent::Done(reason) => {
                self.clear_tool_line();
                self.flush_buffered_markdown();
                if reason == "cancelled" {
                    eprintln!("\n[cancelled]");
                } else if reason == "maxTurnRequests" {
                    eprintln!(
                        "\n\x1b[33m[max tool rounds reached — try rephrasing or using \x1b[35m/compact\x1b[33m]\x1b[0m"
                    );
                } else {
                    println!();
                    if metrics.context_size > 0 {
                        let pct = metrics.used_tokens * 100 / metrics.context_size;
                        eprintln!(
                            "\x1b[90m[tokens: {}/{} ({}%)]\x1b[0m",
                            fmt_tokens(metrics.used_tokens),
                            fmt_tokens(metrics.context_size),
                            pct,
                        );
                    }
                    eprintln!();
                }
                let _ = stdout.flush();
                self.stop = Some(TurnStop::Done { reason });
            }
        }
        cwd_sync
    }

    /// Clear the live `┆` tool line, if any.
    fn clear_tool_line(&mut self) {
        if self.tool_line_active {
            eprint!("\r\x1b[2K\n");
            let _ = std::io::stderr().flush();
            self.tool_line_active = false;
        }
    }

    /// In non-streaming mode, render and print any buffered text as markdown.
    fn flush_buffered_markdown(&mut self) {
        if !self.stream && !self.response_buf.is_empty() {
            reset_display();
            print!("{}", crate::markdown::render(&self.response_buf));
            let _ = std::io::stdout().flush();
            self.response_buf.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn done_signals_stop_with_reason() {
        let mut r = TurnRenderer::new(true);
        let mut m = TurnMetrics {
            used_tokens: 500,
            context_size: 10_000,
        };
        r.handle(StreamEvent::Done("end_turn".into()), &mut m);
        assert!(matches!(r.take_stop(), Some(TurnStop::Done { reason }) if reason == "end_turn"));
    }

    #[test]
    fn tool_finished_with_output_returns_cwd_sync() {
        let mut r = TurnRenderer::new(true);
        let mut m = TurnMetrics::default();
        let sync = r.handle(
            StreamEvent::ToolFinished {
                name: "bash".into(),
                output: "/tmp\n".into(),
                exit_code: Some(0),
                status: ToolCallStatus::Completed,
            },
            &mut m,
        );
        assert!(sync.is_some());
        let sync = sync.unwrap();
        assert_eq!(sync.tool_name, "bash");
        assert_eq!(sync.output, "/tmp\n");
    }

    #[test]
    fn error_signals_stop() {
        let mut r = TurnRenderer::new(true);
        let mut m = TurnMetrics::default();
        r.handle(StreamEvent::Error("boom".into()), &mut m);
        assert!(matches!(r.take_stop(), Some(TurnStop::Error(msg)) if msg == "boom"));
    }
}
